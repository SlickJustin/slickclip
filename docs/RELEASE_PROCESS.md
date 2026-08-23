# SlickClip Signed Release Process

This process is intentionally fail-closed. `npm run bundle` remains the unsigned local packaging command. Only `npm run bundle:release` may produce a release candidate, and it refuses to run without every updater and Windows-signing input.

## Protected release inputs

Provide these as process environment variables from an approved secret store. Never commit or print private key material.

- `SLICKCLIP_UPDATER_ENDPOINT`: absolute HTTPS URL for the update manifest. Tauri variables such as `{{target}}`, `{{arch}}`, and `{{current_version}}` are supported.
- `SLICKCLIP_UPDATER_PUBLIC_KEY`: Minisign public key generated for Tauri updater verification. This is embedded in the release binary and must match the private updater key.
- `SLICKCLIP_UPDATER_ARTIFACT_URL`: absolute HTTPS URL where this version's signed NSIS updater artifact will be published.
- `TAURI_SIGNING_PRIVATE_KEY`: Tauri updater private key or its path, as supported by the Tauri CLI.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: set this when the updater key has a password. Tauri permits it to be unset/empty for an intentionally passwordless key.
- `SLICKCLIP_WINDOWS_SIGN_COMMAND`: approved Tauri Windows `signCommand`, including the required `%1` file placeholder. Its certificate credentials must already be available to the chosen signing tool.
- `SLICKCLIP_RELEASE_NOTES`: optional plain-text release notes for `latest.json`.

Back up the updater private key in the approved recovery store before the first public release. Losing it prevents installed builds from accepting future updates. Replacing the public key requires a signed bridge release trusted by the old key; do not rotate it casually.

## Build and verify

1. Use a clean release-candidate commit and set the intended SemVer consistently in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Run `npm run bundle:release` on the approved Windows release machine.
3. The script re-verifies pinned FFmpeg, creates a temporary Tauri configuration overlay, enables updater artifacts, embeds the HTTPS endpoint/public key, invokes the supplied Windows sign command, and deletes the temporary overlay.
4. The script fails unless Authenticode reports `Valid` for both `SlickClip.exe` and the NSIS installer and the Tauri updater `.sig` exists and is nonempty.
5. The script generates `latest.json` beside the installer and prints SHA-256 hashes for the application and installer. Record those values in the release notes and manual-validation log.

Expected output directory: `src-tauri\target\release\bundle\nsis`.

## Publish safely

Upload the signed versioned installer and its updater signature first. Verify their remote hashes and HTTPS accessibility. Publish `latest.json` last so no client can observe a manifest whose artifact is missing. Never publish the unsigned Stage 25 candidate or a manifest with placeholder URLs, keys, or signatures.

The updater checks SemVer, downloads over HTTPS, verifies the complete installer against the embedded updater public key, then shuts down capture/save/export managers before launching the passive installer. A changed release between Check and Update & Restart is rejected and must be checked again. Signature, network, or manifest failures leave the installed version in place.

The normal updater does not permit downgrades. Recover from a bad public release by publishing a higher fixed SemVer. Any deliberate downgrade/rollback procedure must be separately approved and tested with backed-up disposable data; do not weaken signature or version checks.

## Required manual release-candidate validation

Use disposable clean and upgrade Windows VMs. Test no-update (HTTP 204), malformed manifest, unreachable endpoint, wrong signature, interrupted download, changed available version, successful Update & Restart, preserved clips/settings, startup/tray behavior after the install path changes, and uninstall preservation. Verify Authenticode and updater signatures independently on downloaded artifacts, then complete every item in `RELEASE_GATE.md` before publishing.
