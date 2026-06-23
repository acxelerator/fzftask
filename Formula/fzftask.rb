class Fzftask < Formula
  desc "Terminal UI to fuzzy-find and run Taskfile tasks"
  homepage "https://github.com/acxelerator/fzftask"
  url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.7.tar.gz"
  sha256 "d94d95246ae59f584fede1ccce31a77ebe0aaf1491074b2ecacd108d128dbf9f"
  license "MIT"
  head "https://github.com/acxelerator/fzftask.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    # Ship the zsh integration so users can load picks onto their prompt.
    pkgshare.install "shell/fzftask.zsh"
  end

  def caveats
    <<~EOS
      `fzftask` prints the selected command to stdout. To load it onto your
      shell prompt instead (without running it), source the zsh integration:

        echo 'source #{opt_pkgshare}/fzftask.zsh' >> ~/.zshrc

      Then run `ft` to pick a task — Enter pre-fills your prompt with the
      command. (Ctrl-T inserts it at the cursor on the current line.)
    EOS
  end

  test do
    assert_match "fzftask #{version}", shell_output("#{bin}/fzftask --version")
    assert_path_exists pkgshare/"fzftask.zsh"
  end
end
