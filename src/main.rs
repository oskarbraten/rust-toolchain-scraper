use futures_util::{stream, StreamExt};
use std::io::{Result, Write};
use std::path::{Path, PathBuf};
use url::Url;

const ROOT_DIRECTORY: &'static str = "static";
const RUSTLANG_ROOT_URL: &'static str = "https://static.rust-lang.org";

const ARCHITECTURES: &'static [&'static str] = &[
    "x86_64-apple-darwin",
    "x86_64-linux-android",
    "x86_64-pc-windows-gnu",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-freebsd",
    "x86_64-unknown-illumos",
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "x86_64-unknown-netbsd",
];

async fn download(path: &str) -> Result<()> {
    let url = format!("{}{}", RUSTLANG_ROOT_URL, path);
    println!("Downloading {}...", url);

    match reqwest::get(&url).await {
        Ok(res) => {
            let path = PathBuf::from(format!("{}{}", ROOT_DIRECTORY, path));

            println!("Writing file {}...", path.display());

            if let Some(path) = path.parent() {
                std::fs::create_dir_all(path)?;
            }

            let mut stream = res.bytes_stream();
            let mut file = std::fs::File::create(path)?;

            while let Some(Ok(bytes)) = stream.next().await {
                file.write(&bytes)?;
            }
        }
        Err(error) => {
            println!("Error downloading file: {}", url);
            println!("{}", error);
        }
    }

    Ok(())
}

async fn rustup() -> Result<()> {
    download("/rustup/release-stable.toml").await?;

    for arch in ARCHITECTURES {
        let ext = if arch.contains("windows") { ".exe" } else { "" };
        let name = format!("rustup-init{}", ext);
        let url = format!("/rustup/dist/{}/{}", arch, name);

        download(&url).await?;
    }

    Ok(())
}

async fn dist(channel: &str) -> Result<()> {
    download(&format!("/dist/channel-rust-{}.toml.sha256", channel)).await?;
    download(&format!("/dist/channel-rust-{}.toml.asc", channel)).await?;
    download(&format!("/dist/channel-rust-{}.toml", channel)).await?;

    let path = PathBuf::from(format!(
        "{}/dist/channel-rust-{}.toml",
        ROOT_DIRECTORY, channel
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

            if !ARCHITECTURES.iter().any(|arch| line.contains(arch)) {
                return None;
            }

            let url = Url::parse(line).ok()?;
            if &url.origin().ascii_serialization() == RUSTLANG_ROOT_URL {
                Some(url.path().to_string())
            } else {
                println!(
                    "Skipping URL ({}) in channel manifest that does not have this origin: {}",
                    line, RUSTLANG_ROOT_URL
                );
                None
            }
        })
        .collect();

    let total = pkg_urls.len();
    stream::iter(pkg_urls.iter().enumerate())
        .for_each_concurrent(5, |(i, url)| {
            println!("Downloading – {}/{}", i + 1, total);

            let url = url.to_string();
            async move {
                let _ = download(&url).await;
            }
        })
        .await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let root_directory = Path::new(ROOT_DIRECTORY);
    if !root_directory.has_root() || root_directory.components().count() > 3 {
        let _ = std::fs::remove_dir_all(ROOT_DIRECTORY);
    }

    rustup().await?;
    dist("stable").await?;
    dist("nightly").await?;

    Ok(())
}
