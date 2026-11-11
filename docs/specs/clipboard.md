# Clipboard history (Phase 9, done before Phase 4)

## Goal

A Raycast-style clipboard history. A hotkey (default **Hyper+V**) opens a popup that lists what was copied (text and images), lets you search it, see a preview, paste an entry into the app you were in, and **pin** entries you want to keep.

History is kept **in memory only**: a restart of Kuality (or of the PC) starts with an empty history. **Pinned entries survive restarts.** Everything is encrypted, in memory and on disk.

If the user records **Win+V** as the hotkey, Kuality's keyboard hook swallows it and the Windows clipboard history panel no longer opens.

## Behavior (user-visible)

- **Settings → Clipboard** (new sidebar tab):
  - **Enable clipboard history** (off by default). Off → nothing is recorded, the history in memory is wiped; the pins file is kept.
  - **Hotkey**: the same recorder button as the Shortcuts tab (default `Hyper + V`). Hint: "Record Win + V to replace the Windows clipboard history." A Hyper hotkey while Hyperkey is off shows a warning (as in Shortcuts). A hotkey also used by a shortcut shows a duplicate warning in both tabs; the clipboard hotkey wins.
  - **Keep the last**: 50 / 100 / 200 / 500 entries (segmented; default 100). Pinned entries do not count.
  - **Record images** (on by default).
  - **Open clipboard history**, **Clear history** (unpinned entries) and **Remove all pins** (asks for confirmation) buttons. They talk to the running core and are disabled when it is not running.
- **Recording**:
  - Every clipboard change is recorded: text (`CF_UNICODETEXT`), else an image (`CF_DIBV5` / `CF_DIB`). When both are present (e.g. cells copied from Excel) only the text is kept.
  - Copying something already in the history moves it to the top (newest) instead of adding a copy.
  - **Skipped**: clipboard contents marked private by the source app — formats `ExcludeClipboardContentFromMonitorProcessing`, `Clipboard Viewer Ignore`, or `CanIncludeInClipboardHistory` with value 0 (password managers such as KeePass, Bitwarden, 1Password set these; Windows' own history respects them too). Empty / whitespace-only text is skipped.
  - Limits (not settings): a text entry over 1 MB, or an image over 40 megapixels, is skipped (logged at Info, without content). Unpinned images together keep at most 64 MB (PNG); the oldest unpinned images are dropped first.
- **Popup** (`kuality.exe --clipboard`, a separate GPUI process like the to-do popup):
  - About 720 × 460 DIPs, centered on the work area of the monitor of the window that was in front, rounded, topmost, no taskbar button.
  - Top: a search input ("Search clipboard…", focused). Typing filters text entries (case-insensitive substring); images match "image".
  - Left: the list. **Pinned** group first (pin icon), then **Recent**, newest first. Each row: first line of the text (or a thumbnail + `1920 × 1080`), and a relative time (`now`, `5 min`, `2 h`, `yesterday`).
  - Right: preview of the selected entry: full text (monospace, scrollable) or the image fitted to the pane; below, `Text · 123 characters · 4 lines` or `Image · 1920 × 1080`, "Copied 5 min ago", and pin / delete buttons.
  - Keys: ↑ / ↓ move the selection; **Enter** pastes; **Shift+Enter** copies only; **Ctrl+P** pins / unpins; **Ctrl+Delete** deletes the entry; **Esc** closes. A click selects, a double click pastes; a row has a pin and a delete button on hover.
  - **Paste**: the popup closes, the entry goes to the clipboard, the window that was in front gets the focus back and receives Ctrl+V. **Copy**: the popup closes, the entry is on the clipboard. Opened from Settings ("Open clipboard history"), Enter copies (there is no window to paste into).
  - It closes on Esc, focus loss, and when the hotkey is pressed again while it is open (toggle).
  - Empty history: "Nothing copied yet." With the feature off (opened by hand): "Clipboard history is off — turn it on in Settings."

## Config

`config.json`:

```json
"clipboard": { "enabled": false, "hotkey": "hyper+v", "max_entries": 100, "images": true }
```

- `hotkey` is a trigger string with the Shortcuts syntax (`hyper+v`, `win+v`, `ctrl+shift+v`); `Lenient`. Invalid or reserved → warning in the log, no hotkey (the feature still records).
- `max_entries` is `Lenient<u32>`, clamped to 10–1000 at runtime.
- `extra` kept, `#[serde(default)]`, as every section.

Pins file `clipboard-pins.bin` (data dir, next to `config.json`):

- The whole file is a DPAPI blob (`CryptProtectData`, current user, `CRYPTPROTECT_UI_FORBIDDEN`, fixed app entropy `Kuality.ClipboardPins.v1`).
- Decrypted payload: `KCLP` magic, `u32` version 1, `u32` header length, JSON header `{ "entries": [ { "id", "kind": "text"|"image", "created", "pinned_at", "len", "width", "height" } ] }`, then the payloads (UTF-8 text / PNG) back to back in header order.
- Missing file = no pins. Undecryptable or invalid file → warning, no pins loaded, and the file is **not** overwritten until the user pins or unpins something (then it is replaced).

## Implementation notes

- **Core, only while `clipboard.enabled`** (both threads start on enable, stop on disable or quit):
  - Thread `kuality-clipboard`: a message-only window with `AddClipboardFormatListener`. On `WM_CLIPBOARDUPDATE` it skips its own writes (`GetClipboardOwner` = our window), checks the private markers, then reads text or the DIB and encodes the image as PNG (`png` crate, on this thread). `OpenClipboard` failing (another app holds it) is retried by a timer at 50, 100, 200, 400 ms, then given up (logged at Debug). `GetClipboardSequenceNumber` avoids reading the same change twice.
  - It owns the `History` (portable logic in `core/src/clipboard/history.rs`: ids, dedup by SHA-256 of kind + payload, ordering, max entries, image budget, pins; unit-tested, no OS types). Payloads are stored through a `Sealed` buffer: padded to 16 bytes and encrypted in place with `CryptProtectMemory(CRYPTPROTECTMEMORY_SAME_PROCESS)`; `open()` returns a decrypted copy that is zeroed on drop (`zeroize`). The dedup hash is kept, not the plain text. Search previews (first 200 characters) and thumbnails are sealed too.
  - Pins: loaded at enable, saved (atomic `store::write_atomic`) after every pin / unpin / delete of a pinned entry / "Remove all pins".
  - Thread `kuality-ipc`: named pipe server `\\.\pipe\Kuality.Core.<core pid>` (byte mode, one connection at a time, one request per connection; clients know the core pid from `--core-pid` or its window). Security: DACL granting only the current user's SID and SYSTEM (`D:P(A;;GA;;;<user>)(A;;GA;;;SY)`, medium integrity label so a non-elevated client reaches an elevated core), `PIPE_REJECT_REMOTE_CLIENTS`, `FILE_FLAG_FIRST_PIPE_INSTANCE`; a client whose process image (`GetNamedPipeClientProcessId` + `QueryFullProcessImageNameW`) is not our own exe is disconnected. This is the first piece of the deferred "Named pipe IPC" item; settings changes keep going through `config.json`.
  - Protocol (`protocol::ipc`): frames are `u32` little-endian length + JSON header + optional binary payload (`u32` length prefix). Requests: `list` → metadata, previews and thumbnails (PNG ≤ 96 px); `get {id}` → full text / PNG; `paste {id}`, `copy {id}`, `pin {id}`, `unpin {id}`, `delete {id}`, `clear`, `clear_pins`. Replies `ok` / `error {message}`. The history is shared by the two threads behind a mutex; only writing to the clipboard is handed to the clipboard thread (channel + `PostMessageW`, 3 s timeout), because the clipboard needs its window as owner.
- **Hotkey**: `shortcuts::compile` takes the clipboard section too and puts a built-in entry `CompiledAction::OpenClipboard` (source shown as `clipboard hotkey` in logs) **before** the user bindings, so it wins duplicates; it works when `shortcuts.enabled` is off. A Win hotkey goes through the existing normal-hotkey path (swallowed, mask key sent). The keyboard thread posts `WM_APP_CLIPBOARD` to the main window → `Event::Clipboard` → `App` remembers `GetForegroundWindow()` as the paste target and spawns `kuality.exe --clipboard --core-pid <pid> --paste` (`spawn_detached`, after `AllowSetForegroundWindow`), or closes the popup if its mutex `Local\Kuality.Clipboard` exists (500 ms start guard, as for to-do).
- **Paste**: on `paste {id}` the clipboard thread puts the entry on the clipboard (text: `CF_UNICODETEXT`; image: `CF_DIB` + registered `PNG` format), the IPC thread replies `ok`, and the popup quits. The IPC thread then waits for the popup process to exit (≤ 1.5 s), brings back the window remembered at hotkey time (`claim_last_input` + `SetForegroundWindow`, only if it still exists, checked for up to 300 ms), and asks the keyboard thread to send Ctrl+V (same path as `send_hotkey`). Our own clipboard write is not recorded again (`GetClipboardOwner`); the entry just moves to the top.
- **Popup** (`settings/src/clipboard.rs`, GPUI): `list` is read before GPUI starts (no empty first frame). Same setup as the to-do popup (`WindowKind::PopUp`, `ui::style`, `popup_window_setup`, closes on deactivation after 400 ms and on Esc, quits when the core exits, mutex `Local\Kuality.Clipboard`, title `Kuality Clipboard`). It connects to the pipe, checks the server PID (`GetNamedPipeServerProcessId`) is the core's, asks `list`, and `get` for the selected entry. It keeps decrypted data only in its own memory while open; nothing is written to disk.
- **Settings**: `Tab::Clipboard` (Lucide `Clipboard` icon via `icon_assets!`), `ui/clipboard.rs`. The recorder gets a `RecordTarget::ClipboardHotkey`; `hotkey_button` is generalised to take a target and an element id. Buttons use the same pipe client as the popup.

## Edge cases

- **Elevated window in front**: the hook does not see the keys, so the hotkey does nothing there (and Win+V opens the Windows panel); pasting into an elevated app fails silently unless Kuality runs as administrator. Documented limitation (same as Shortcuts).
- **Core runs elevated**: the popup is started by the core and runs elevated too; the pipe DACL still allows the same user.
- **Remembered window gone** when pasting: the entry is put on the clipboard only (like Copy).
- **Clipboard held by another app** for more than ~750 ms: that change is not recorded.
- **Delayed rendering** (apps that render on request): reading the clipboard asks the source app to render; that happens on the clipboard thread, never on the main or keyboard thread.
- **Feature disabled while the popup is open**: the pipe closes; the popup shows "Clipboard history is off" and closes on the next key.
- **Lock / sleep / resume**: nothing special; the listener keeps working. Explorer restart does not affect the listener.
- **Pins file from another user / PC** (copied folder, roaming): cannot be decrypted → no pins, file left alone until the next pin change.

## Out of scope

- Rich text / HTML / RTF (only plain text is kept), copied files (`CF_HDROP`), colors, links with titles.
- An app ignore list, clear-on-lock, a "keep popup ready" mode (GPUI start ≈ 0.3–0.5 s), pinned groups / ordering, editing entries, sync.
- Moving settings changes to the named pipe.

## Security summary

- Other users and anyone with the files (backup, copied folder, another PC) cannot read pins: DPAPI ties them to this Windows account.
- History never touches the disk; in memory it is encrypted with a key that only exists inside the running Kuality process, so a memory dump or a scan of Kuality's memory does not show clipboard contents in plain text.
- Private clipboard contents (password managers) are never recorded.
- Only our own exe, running as the same user, can talk to the pipe.
- Not protected against malware running as the same user: such a program could read the clipboard directly anyway.

## Test plan

- **Automated**:
  - protocol: `clipboard` section defaults and round trip, `Lenient` invalid values kept; IPC frame encode/decode (truncated, oversized, with/without payload); pins payload encode/decode.
  - core: `History` (dedup moves to top, max entries ignores pins, image budget drops oldest unpinned images, pin/unpin/delete/clear, ids); `Sealed` round trip; `CryptProtectMemory` and DPAPI round trips, tampered blob → error; DIB / PNG conversion and thumbnails; `shortcuts::compile` with the built-in entry (first wins, works with shortcuts off, invalid hotkey skipped).
  - settings: popup placement (centered, scale), search filter, relative time formatting.
- **Manual** (portable `target\debug`, 2026-11-11):
  - [x] Recording: two texts, the first copied again (moved up, not added twice), an image (DIB → PNG); log shows only kinds and ids.
  - [x] Popup opens centered on the monitor of the window in front and closes on focus loss; clean shutdown of both threads at quit (`IPC pipe closed`, `clipboard history: stopped`).
  - [ ] Enable in Settings → copy three texts and a screenshot → Hyper+V opens the popup with all four, newest first.
  - [ ] Search filters; Enter pastes into Notepad; Shift+Enter only copies; double click pastes.
  - [ ] Pin one text and the screenshot → restart the core → only the pins are there.
  - [ ] Record Win+V → the Windows panel no longer opens, ours does; back to Hyper+V → Win+V opens the Windows panel again.
  - [ ] Copy a password from KeePass / Bitwarden → not recorded.
  - [ ] `clipboard-pins.bin` opened in a hex editor shows no readable text.
  - [ ] Core working set idle with 100 text entries + a few screenshots; ~0 % CPU when idle.
  - [ ] Disable the feature → log shows both threads stopped, popup says it is off.
  - [ ] Explorer restart and sleep/resume with the feature on: still records.

## Assumptions

- One history per Windows session (the core is single-instance per session).

## Dependencies

- One new direct dependency of the core, already in `Cargo.lock` through GPUI: `png` (encode / decode clipboard images). Decrypted buffers are wiped by a small volatile-write helper (no `zeroize` crate). Duplicates use std's `RandomState` (SipHash with a random per-process key; the hash is compared, then the decrypted bytes) instead of SHA-256.
- `windows` features, core: `Win32_System_DataExchange`, `Win32_System_Memory`, `Win32_System_Ole`, `Win32_System_Pipes`, `Win32_Security_Authorization`, `Win32_Security_Cryptography`; settings: `Win32_System_Pipes` (the client opens the pipe with `std::fs`).
