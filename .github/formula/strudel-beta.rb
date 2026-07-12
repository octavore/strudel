class StrudelBeta < Formula
  desc "Build, sign, notarize, and package macOS Swift apps (pre-release channel)."
  homepage "https://github.com/octavore/strudel"
  version "__VERSION__"
  license "Apache-2.0"

  conflicts_with "strudel", because: "strudel-beta installs the same binaries as strudel"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/octavore/strudel/releases/download/v__VERSION__/__ARCHIVE__"
      sha256 "__SHA256__"
    elsif Hardware::CPU.intel?
      odie "strudel is not supported on Intel macs"
    end
  end

  def install
    bin.install "strudel"
  end

  test do
    system bin/"strudel", "--help"
  end
end
