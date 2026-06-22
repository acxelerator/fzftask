class Fzftask < Formula
  desc "Terminal UI to fuzzy-find and run Taskfile tasks"
  homepage "https://github.com/acxelerator/fzftask"
  url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "2b628c087588767fe1fa074d619545190b9c72ada5bba67085c74f01f8b7d961"
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
