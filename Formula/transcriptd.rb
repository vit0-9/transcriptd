class Transcriptd < Formula
  desc "Capture, index and search your AI coding conversations"
  homepage "https://github.com/vit0-9/transcriptd"
  version "0.1.0"
  license "AGPL-3.0-or-later"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/vit0-9/transcriptd/releases/download/v#{version}/transcriptd-macos-arm64.tar.gz"
      # sha256 "PLACEHOLDER"
    else
      url "https://github.com/vit0-9/transcriptd/releases/download/v#{version}/transcriptd-macos-amd64.tar.gz"
      # sha256 "PLACEHOLDER"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/vit0-9/transcriptd/releases/download/v#{version}/transcriptd-linux-arm64.tar.gz"
      # sha256 "PLACEHOLDER"
    else
      url "https://github.com/vit0-9/transcriptd/releases/download/v#{version}/transcriptd-linux-amd64.tar.gz"
      # sha256 "PLACEHOLDER"
    end
  end

  def install
    bin.install "transcriptd"
  end

  test do
    system "#{bin}/transcriptd", "--version"
  end
end
