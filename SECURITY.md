# Security Policy

## Reporting a vulnerability

Please report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/cubiix3/Kova-Screen/security/advisories/new)
rather than opening a public issue.

Include what you can: affected version, Windows build, steps to reproduce, and
what an attacker gains. A proof of concept helps but is not required.

Expect an acknowledgement within a few days. Supported version: the latest
release.

## Threat model

Kova Screen is a local desktop application. It has no server component, no
account system and no telemetry, so the interesting attack surface is small.

| Surface | Handling |
| --- | --- |
| The vgy.me user key | Stored in Windows Credential Manager, never in a config file. Write-only across the app's internal boundary: it can be saved and its presence queried, but no code path returns it to the UI. Never logged, never in an error message. |
| Upload deletion links | A capability — anyone holding one can delete the upload. Stored locally, excluded from logs, from `Debug` output and from everything sent to the frontend. Only followed when they point at the provider's own host. |
| Upload requests | HTTPS only; a non-`https` endpoint is refused outright. Bounded connect and response timeouts, a redirect limit, a maximum upload size, and a cap on how much of a response is read. |
| Response parsing | Every field beyond the error flag is optional, so a changed or hostile response produces a message rather than a panic. Server-supplied text is length-capped before it reaches a toast. |
| Filenames | The filename template is user input that becomes a path. It is reduced to a single path component: separators, control characters and Windows-reserved device names are rewritten, so a template cannot traverse out of the capture folder. |
| Capture size | Captures are bounds-checked before allocation, so a malformed monitor rect or a hostile window extent cannot become a huge allocation. |
| IPC | Commands that act on a file take a history id, never a path, so a crafted call cannot make the app open or delete an arbitrary file. |
| The multipart body | The filename is escaped before it enters a header, so it cannot inject header lines or break out of its quoting. |

Tests assert the secret-handling properties above. If you find a path that leaks
one, that is a valid report even without a full exploit.

## Out of scope

- An attacker who already has code execution as your user. They can read the
  credential vault, the capture folder and the clipboard directly; nothing here
  defends against that.
- Screenshots containing sensitive information. Kova Screen captures what you
  point it at and does not inspect the contents.
- The security of vgy.me itself, or of anything you choose to upload there.

## Dependencies

Dependabot alerts and security updates are enabled. Dependencies are kept few
and deliberately boring; the notable ones are `tauri`, `windows`, `image`,
`rusqlite`, `ureq` and `rustls`.

### Linux-only transitive dependencies

`Cargo.lock` resolves dependencies for every platform Tauri supports, not just
the one we ship. Tauri's tray implementation pulls in the GTK stack — `gtk`,
`atk`, `glib`, `libappindicator` — for Linux. Those crates are behind a target
gate and are **never compiled into the Windows binary**:

```bash
# Lists the crate and its dependents on Linux
cargo tree -i glib --target all

# Prints "nothing to print" for the target we actually ship
cargo tree -i glib --target x86_64-pc-windows-msvc
```

Advisories against those crates are reported by Dependabot, because it reads
the lockfile rather than the build graph. They do not affect a Kova Screen
release. Check an alert against the Windows target before treating it as
exploitable here.

### GHSA-wrw7-89jp-8q8g (`glib` 0.18)

[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html)
affects `glib::VariantStrIter` in `glib` 0.15.0 through 0.19. The lockfile
resolves `glib` 0.18.5 because Tauri 2 still depends on GTK 3 (`gtk` 0.18,
`webkit2gtk` 2.0). That line is only fixed in `glib` 0.20, which does not
build against GTK 3, and current stable Tauri has not moved to GTK 4.

`cargo tree -i glib --target x86_64-pc-windows-msvc` prints nothing. The
Windows installer does not compile or ship `glib`. The Dependabot alert is
dismissed as not used for that reason, and it should be revisited when a
stable Tauri release leaves `gtk` 0.18.
