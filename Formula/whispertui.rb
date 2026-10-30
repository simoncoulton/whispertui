class Whispertui < Formula
  desc "Hotkey voice-to-text for macOS using local whisper.cpp"
  homepage "https://github.com/simoncoulton/whispertui"
  version "0.1.0"
  license "MIT"

  depends_on "whisper-cpp"

  on_macos do
    on_arm do
      url "https://github.com/simoncoulton/whispertui/releases/download/v0.1.0/whispertui-v0.1.0-aarch64-apple-darwin.tar.gz"
      sha256 "c8a0e3b5098273c9cabdb10eaf0684bc487fa70d81a46bea6bcee86b02bf38c1"
    end
    on_intel do
      url "https://github.com/simoncoulton/whispertui/releases/download/v0.1.0/whispertui-v0.1.0-x86_64-apple-darwin.tar.gz"
      sha256 "d014253e6f0697f33eba19ea74cb8d874b2b9c28b83f45b86f979340bd18f187"
    end
  end

  def install
    bin.install "whispertui"
  end

  test do
    system bin/"whispertui", "--help"
  end
end
