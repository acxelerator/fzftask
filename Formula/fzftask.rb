class Fzftask < Formula
  desc "Terminal UI to fuzzy-find and run Taskfile tasks"
  homepage "https://github.com/acxelerator/fzftask"
  url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.1.tar.gz"
  sha256 "2bf8c2f494f05343c147bdb921cc0e09b1dd782aa9682ddf0facd64c8842465b"
  license "MIT"
  head "https://github.com/acxelerator/fzftask.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "fzftask #{version}", shell_output("#{bin}/fzftask --version")
  end
end
