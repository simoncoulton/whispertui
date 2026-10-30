class Whispertui < Formula
  desc "Hotkey voice-to-text for macOS using local whisper.cpp"
  homepage "https://github.com/simoncoulton/whispertui"
  version "0.1.0"
  license "MIT"

  depends_on "whisper-cpp"

  on_macos do
    on_arm do
      url "https://github.com/simoncoulton/whispertui/releases/download/v0.1.0/whispertui-v0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/simoncoulton/whispertui/releases/download/v0.1.0/whispertui-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "whispertui"
  end

  test do
    system bin/"whispertui", "--help"
  end
end
