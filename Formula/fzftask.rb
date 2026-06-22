class Fzftask < Formula
  desc "Terminal UI to fuzzy-find and run Taskfile tasks"
  homepage "https://github.com/acxelerator/fzftask"
  url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.0.tar.gz"
  # Fill in after creating the v0.1.0 release:
  #   curl -sL https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
  sha256 ""
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
