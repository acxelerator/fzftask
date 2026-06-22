class Fzftask < Formula
  desc "Terminal UI to fuzzy-find and run Taskfile tasks"
  homepage "https://github.com/acxelerator/fzftask"
  url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "0ffd39fb2f23d326afbe02d7c7b0c05d8a43cfa1b7d29824d5e1736ffb63320a"
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
