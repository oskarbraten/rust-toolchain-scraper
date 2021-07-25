use clap::{App, Arg};
use crates_index::Index;
use futures_util::{stream, StreamExt};
use log::LevelFilter;
use regex::Regex;
use reqwest::Client;
use sha2::{Digest, Sha256};
use simple_logger::SimpleLogger;
use std::collections::HashSet;
use std::convert::TryInto;
use std::io::{Result, Write};
use std::path::PathBuf;
use url::Url;

const RUSTLANG_ROOT_URL: &'static str = "https://static.rust-lang.org";
const CRATES_ROOT_URL: &'static str = "https://static.crates.io";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overwrite {
    True,
    False,
    Checksum([u8; 32]),
}

async fn download(
    http_client: &Client,
    output_directory: &str,
    path: &str,
    overwrite: Overwrite,
) -> Result<()> {
    let url = if path.ends_with(".crate") {
        format!("{}{}", CRATES_ROOT_URL, path)
    } else {
        format!("{}{}", RUSTLANG_ROOT_URL, path)
    };

    let path_buf = PathBuf::from(format!("{}{}", output_directory, path));

    let download = overwrite == Overwrite::True
        || !path_buf.exists()
        || !(match overwrite {
            Overwrite::Checksum(checksum) => {
                let bytes = std::fs::read(&path_buf)?;
                let digest: [u8; 32] = Sha256::digest(&bytes).as_slice().try_into().unwrap();
                checksum == digest
            }
            Overwrite::False => true,
            Overwrite::True => unreachable!(), // Convered by short-circuit in first clause.
        });

    if download {
        log::info!("Downloading {}...", url);
        match http_client.get(&url).send().await {
            Ok(res) => {
                log::debug!("Writing file {}...", path_buf.display());

                if let Some(path) = path_buf.parent() {
                    std::fs::create_dir_all(path)?;
                }

                let mut stream = res.bytes_stream();
                let mut file = std::fs::File::create(path_buf)?;

                while let Some(Ok(bytes)) = stream.next().await {
                    file.write(&bytes)?;
                }
            }
            Err(error) => {
                log::warn!("Error downloading file: {}", url);
                log::debug!("{}", error);
            }
        }
    }

    Ok(())
}

async fn rustup(
    http_client: &Client,
    output_directory: &str,
    concurrency: usize,
    architectures: &Vec<String>,
) -> Result<()> {
    log::info!("Downloading rustup executables...");
    download(
        http_client,
        output_directory,
        "/rustup/release-stable.toml",
        Overwrite::True,
    )
    .await?;

    stream::iter(architectures.iter())
        .for_each_concurrent(concurrency, |arch| {
            let ext = if arch.contains("windows") { ".exe" } else { "" };
            let name = format!("rustup-init{}", ext);
            let url = format!("/rustup/dist/{}/{}", arch, name);

            async move {
                let _ = download(http_client, output_directory, &url, Overwrite::True).await;
            }
        })
        .await;

    Ok(())
}

async fn get_dist_archiectures(
    http_client: &Client,
    output_directory: &str,
    channel: &str,
) -> Result<Vec<String>> {
    log::info!(
        "Getting all available architectures for the Rust toolchain [channel-{}]...",
        channel
    );

    download(
        http_client,
        output_directory,
        &format!("/dist/channel-rust-{}.toml", channel),
        Overwrite::True,
    )
    .await?;

    let path = PathBuf::from(format!(
        "{}/dist/channel-rust-{}.toml",
        output_directory, channel
    ));

    let manifest = std::fs::read_to_string(path)?;

    let architectures: HashSet<String> = manifest
        .lines()
        .filter_map(|line| {
            let line = line.trim();

            if !line.starts_with("target = ") {
                return None;
            }

            let mut iter = line.chars();

            iter.find(|c| *c == '"'); // Trim off characters from the front until the first double-quote.
            let line = iter.as_str().trim_end_matches('"'); // Trim off double-quote at the end of the line.

            Some(line.to_string())
        })
        .collect();

    Ok(architectures.into_iter().collect())
}

async fn dist_download(
    http_client: &Client,
    output_directory: &str,
    path: &str,
    overwrite: Overwrite,
) -> Result<()> {
    download(http_client, output_directory, path, overwrite).await?;
    download(
        http_client,
        output_directory,
        &format!("{}.asc", path),
        overwrite,
    )
    .await?;
    download(
        http_client,
        output_directory,
        &format!("{}.sha256", path),
        overwrite,
    )
    .await
}

async fn dist(
    http_client: &Client,
    output_directory: &str,
    concurrency: usize,
    channel: &str,
    architectures: &Vec<String>,
) -> Result<()> {
    log::info!("Downloading Rust toolchain [channel-{}]...", channel);

    dist_download(
        http_client,
        output_directory,
        &format!("/dist/channel-rust-{}.toml", channel),
        Overwrite::True,
    )
    .await?;

    let path = PathBuf::from(format!(
        "{}/dist/channel-rust-{}.toml",
        output_directory, channel
    ));

    let manifest = std::fs::read_to_string(path)?;

    let pkg_urls: Vec<String> = manifest
        .lines()
        .filter_map(|line| {
            let line = line.trim();

            if !line.starts_with("url") && !line.starts_with("xz_url") {
                return None;
            }

            let mut iter = line.chars();

            iter.find(|c| *c == '"'); // Trim off characters from the front until the first double-quote.
            let line = iter.as_str().trim_end_matches('"'); // Trim off double-quote at the end of the line.

            if !architectures.iter().any(|arch| line.contains(arch)) {
                return None;
            }

            let url = Url::parse(line).ok()?;
            if &url.origin().ascii_serialization() == RUSTLANG_ROOT_URL {
                Some(url.path().to_string())
            } else {
                log::warn!(
                    "Skipping URL ({}) in channel manifest that does not have this origin: {}",
                    line,
                    RUSTLANG_ROOT_URL
                );
                None
            }
        })
        .collect();

    let total = pkg_urls.len();
    stream::iter(pkg_urls.iter().enumerate())
        .for_each_concurrent(concurrency, |(i, url)| {
            log::info!("Downloading – {}/{}", i + 1, total);

            let url = url.to_string();
            async move {
                let _ = dist_download(http_client, output_directory, &url, Overwrite::False).await;
            }
        })
        .await;

    Ok(())
}

async fn crates(
    http_client: &Client,
    output_directory: &str,
    concurrency: usize,
    validate_checksums: bool,
) -> Result<()> {
    let index = Index::new(format!("{}/index", output_directory));

    log::info!("Retrieving/updating crates.io-index...");
    index
        .retrieve_or_update()
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;

    let crates = index
        .crates()
        .filter_map(|c| {
            if c.versions().len() < 2 {
                return None;
            }

            Some(
                c.versions()
                    .iter()
                    .filter(|v| !v.is_yanked())
                    .map(|v| {
                        (
                            v.name().to_string(),
                            v.version().to_string(),
                            v.checksum().clone(),
                        )
                    })
                    .collect::<Vec<(String, String, [u8; 32])>>(),
            )
        })
        .flatten();

    stream::iter(crates.enumerate())
        .for_each_concurrent(concurrency, |(i, (name, version, checksum))| async move {
            let path = format!("/crates/{}/{}-{}.crate", name, name, version);
            log::info!("Checking {}-{} – {}", name, version, i + 1);

            let overwrite = if validate_checksums {
                Overwrite::Checksum(checksum)
            } else {
                Overwrite::False
            };

            let _ = download(http_client, output_directory, &path, overwrite).await;
        })
        .await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let matches = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .about(
            "Downloads the Rust toolchain, the Crates package registry, and rustup for offline use.",
        )
        .arg(
            Arg::new("channels")
            .long("channels")
            .short('d')
            .default_values(&["stable"])
            .about("Specify toolchain channels, versions or dates (possible values: stable|beta|nightly|<major.minor>|<major.minor.patch>|<YYYY-MM-DD>)."),
        )
        .arg(
            Arg::new("verbose")
            .long("verbose")
            .short('v')
            .about("Enable verbose mode."),
        )
        .arg(
            Arg::new("targets")
            .long("targets")
            .short('t')
            .default_value("x86_64")
            .about("Include only toolchain distributions and rustup executables that match this regular expression. Use \"*\" to include rust-src."),
        )
        .arg(
            Arg::new("concurrency")
            .long("concurrency")
            .short('c')
            .default_value("5")
            .about("Maximum number of concurrent HTTP-requests."),
        )
        .arg(
            Arg::new("validate-checksums")
            .long("validate-checksums")
            .about("Enable checksum (SHA-256) validation of existing crate files.")
        )
        .arg(
            Arg::new("user-agent")
            .long("user-agent")
            .default_value("squire (https://github.com/oskarbraten/squire)")
        )
        .arg(
            Arg::new("OUTPUT-DIRECTORY")
            .about("Specifies the output directory for the mirror.")
            .required(true)
            .index(1),
        )
        .get_matches();

    SimpleLogger::new()
        .with_level(if matches.is_present("verbose") {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        })
        .init()
        .unwrap();

    let output_directory = matches.value_of("OUTPUT-DIRECTORY").unwrap();
    let channels = matches.values_of("channels").unwrap();
    let targets_regex = Regex::new(matches.value_of("targets").unwrap()).unwrap();
    let concurrency: usize = matches.value_of_t("concurrency").unwrap();
    let user_agent = matches.value_of("user-agent").unwrap();
    let validate_checksums = matches.value_of("validate-checksums").is_some();

    let http_client = Client::builder()
        .user_agent(user_agent)
        .build()
        .expect("Unable to build reqwest Client!");

    // Filter architectures based on regex:
    let architectures: Vec<String> =
        get_dist_archiectures(&http_client, output_directory, "stable")
            .await?
            .into_iter()
            .filter(|arch| targets_regex.is_match(arch))
            .collect();

    log::info!(
        "Selected architectures [channel-stable]: {}",
        architectures.join(", ")
    );

    // Download rustup executables and manifest:
    rustup(&http_client, output_directory, concurrency, &architectures).await?;

    // Download Rust toolchain(s) and channel manifest:
    for channel in channels {
        dist(
            &http_client,
            output_directory,
            concurrency,
            channel,
            &architectures,
        )
        .await?;
    }

    // Download crate.io-index and crates:
    crates(
        &http_client,
        output_directory,
        concurrency,
        validate_checksums,
    )
    .await?;

    Ok(())
}
