#!/usr/bin/env bash
# Usage: gen-formula.sh <version> <sha-aarch64-apple-darwin> <sha-x86_64-apple-darwin>
# Writes Formula/whispertui.rb pointing at the binaries attached to the GitHub release.
set -euo pipefail
version=$1
base="https://github.com/simoncoulton/whispertui/releases/download/v${version}/whispertui-v${version}"

cat > "$(dirname "$0")/../Formula/whispertui.rb" <<RUBY
class Whispertui < Formula
  desc "Hotkey voice-to-text for macOS using local whisper.cpp"
  homepage "https://github.com/simoncoulton/whispertui"
  version "${version}"
  license "MIT"

  depends_on "whisper-cpp"

  on_macos do
    on_arm do
      url "${base}-aarch64-apple-darwin.tar.gz"
      sha256 "${2}"
    end
    on_intel do
      url "${base}-x86_64-apple-darwin.tar.gz"
      sha256 "${3}"
    end
  end

  def install
    bin.install "whispertui"
  end

  test do
    system bin/"whispertui", "--help"
  end
end
RUBY
