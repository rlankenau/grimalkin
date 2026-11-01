# Grimalkin

Grimalkin is a GPU-accelerated terminal emulator written in Rust, aimed at Windows 10/11 (ConPTY) with the kitty graphics and keyboard protocols. It also builds and runs on macOS and Linux, which is where it is mostly developed. It is early software: the core works, many conveniences are missing.

## Documentation

- [Installing on Windows](https://github.com/rlankenau/grimalkin/blob/trunk/README.md#installing-on-windows)
- [Installing on macOS](https://github.com/rlankenau/grimalkin/blob/trunk/README.md#installing-on-macos)
- [Skins & themes](https://github.com/rlankenau/grimalkin/blob/trunk/README.md#skins--themes)
- [WinAMP skins](https://github.com/rlankenau/grimalkin/blob/trunk/README.md#winamp-skins)
- [Windows Code Signing](Windows-Code-Signing): end-to-end setup of Authenticode signing, with SignPath Foundation (free, recommended) or an AWS KMS key

## Release process

1. Push a `v*` tag on a commit that is on `trunk`.
2. CI builds and publishes the Windows installers and portable zips (x64 and arm64) and the macOS dmgs to the GitHub release.
3. The arm64 dmg is notarized locally: run `scripts/make-app-bundle.sh` with `GRIMALKIN_NOTARY_PROFILE` set to your `notarytool` keychain profile name.
4. Windows signing is controlled by a gate job. With the repository variable `WINDOWS_SIGNING_ENABLED` set to `true`, a `v*` tag push on `rlankenau/grimalkin` is signed through the provider named by `WINDOWS_SIGNING_PROVIDER` (`signpath`, or `kms` = AWS KMS, the default); otherwise the release gets `-unsigned` assets. See [Windows Code Signing](Windows-Code-Signing).
