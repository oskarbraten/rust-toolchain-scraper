# Squire
Downloads the Rust toolchain, the Cargo package registry, and rustup for offline use.
---

## Installation

```bash
git clone https://github.com/oskarbraten/squire.git
cd squire

cargo install .
```

## Usage

```bash
# Create a mirror at ~/Downloads/mirror and limit archiectures to x86_64 Linux GNU using regex.
squire ~/Downloads/mirror -t 'x86_64.*linux-gnu$'
```

## Mirror

The mirror produced consists of four directories:

 - `rustup` – Pretty self explanatory.
 - `dist` – Contains the Rust toolchain.
 - `index` – Crates.io-index (git, as expected).
 - `crates` – All the crates present in the index, with the exception of crates:
   - With less than two published versions
   - With a version number larger than 9999 (in either patch, minor, major)
   - That have been yanked
