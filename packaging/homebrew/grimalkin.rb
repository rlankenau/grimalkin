cask "grimalkin" do
  arch arm: "arm64", intel: "x86_64"

  version "1.0.0-rc3"
  sha256 arm:   "acf57be90a97b02c161da9f256615da3f7ddb7a0e26b6b5fd9c53d18c4bbf8fb",
         intel: "d1463b1141fd89cad6f46681addb8c84747dc01e0f2b1a0bc6d7283d06843c2b"

  url "https://github.com/rlankenau/grimalkin/releases/download/v#{version}/grimalkin-#{version}-macos-#{arch}.dmg"
  name "grimalkin"
  desc "GPU-accelerated terminal emulator with kitty graphics and keyboard protocols"
  homepage "https://github.com/rlankenau/grimalkin"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app needs macOS 11 (LSMinimumSystemVersion). That is below the oldest
  # macOS current Homebrew supports, so a `macos: ">= :big_sur"` constraint would
  # be redundant (the Homebrew/OSDependsOn style cop rejects it).
  depends_on :macos

  app "grimalkin.app"
  # The app bundle's executable doubles as the command-line entry point
  # (`grimalkin --working-directory DIR`, `--list-themes`, ...).
  binary "#{appdir}/grimalkin.app/Contents/MacOS/grimalkin"

  zap trash: [
    "~/.config/grimalkin",
    "~/Library/Application Support/grimalkin",
    "~/Library/Caches/grimalkin",
    "~/Library/Saved Application State/dev.grimalkin.terminal.savedState",
  ]
end
