# Installation

## Homebrew (macOS/Linux)

```sh
brew tap vit0-9/tap
brew install transcriptd
```

## Cargo (from source)

```sh
cargo install --git https://github.com/vit0-9/transcriptd.git
```

## Docker

```sh
docker run -v ~/.local/share/transcriptd:/data ghcr.io/vit0-9/transcriptd stats
```

## Binary download

Download from [Releases](https://github.com/vit0-9/transcriptd/releases).

## Build from source

```sh
git clone https://github.com/vit0-9/transcriptd.git
cd transcriptd
cargo build --release
# Binary at ./target/release/transcriptd
```
