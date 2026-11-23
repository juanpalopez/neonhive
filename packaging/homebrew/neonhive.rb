# Source of truth for the formula in the juanpalopez/homebrew-tap repository.
# scripts/render-homebrew-formula.sh rewrites url and sha256 for a new version;
# edit everything else here, not in the tap. See docs/RELEASING.md.
class Neonhive < Formula
  desc "Cyberpunk terminal UI that visualizes AI coding agents and their subagents"
  homepage "https://github.com/juanpalopez/neonhive"
  url "https://github.com/juanpalopez/neonhive/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "e8aafecdce8448a9710384f71797566be4d89a03c2ea811abc1a7e05345bee6f"
  license "MIT"
  head "https://github.com/juanpalopez/neonhive.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "neonhive #{version}", shell_output("#{bin}/neonhive --version")
    # A headless frame of the scripted demo: needs no terminal and no sessions.
    frame = shell_output("#{bin}/neonhive --demo --snapshot 5 --size 100x30")
    assert_match "NEONHIVE", frame
  end
end
