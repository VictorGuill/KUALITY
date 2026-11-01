# Shortcuts (Phase 2)

## Goal

A hotkey runs a configured action. Initial actions: `launch`, `send_hotkey`, `open_url`. Bound shortcuts are consumed; unbound keys reach apps as the normal Hyper combo, except the Office letters (W/X/P/O/T/D/N/L/Y), which are dropped (hyperkey.md rule 7). Builds directly on [hyperkey.md](hyperkey.md).

*(2026-11-01, follow-up "any hotkey")*: a hotkey is either **Hyper + key** (as in Phase 2) or a **normal combination** of Ctrl/Shift/Alt/Win + key (`ctrl+t`, `win+e`, `ctrl+alt+k`). A bound hotkey is swallowed by default; per binding, `pass_through` lets it reach the app too. See "Any hotkey" below; the rest of this spec describes Hyper hotkeys unless it says otherwise.

## Behavior (user-visible)

- Hyper shortcuts work only while Hyperkey is **active** (`hyperkey.enabled`, not paused) **and** `shortcuts.enabled` is true. Normal hotkeys only need `shortcuts.enabled`. Pause stops all of them.
- Hold CapsLock, press a bound key → the action runs **once**, on key down. Holding the key does not repeat it. The key (down, repeats, up) never reaches any app.
- Hold CapsLock, press an unbound key → apps receive Ctrl+Shift+Alt+Win+key (Phase 1 behavior); an unbound Office letter is dropped instead.
- Physical Ctrl/Shift/Alt/Win held during Hyper are ignored (Phase 1), so Hyper+Shift+1 triggers the Hyper+1 binding.
- Releasing CapsLock before the bound key is fine: the key's up is still swallowed.
- Invalid bindings are skipped with a log warning; the rest keep working.

## Config

```json
"shortcuts": {
  "enabled": false,
  "bindings": [
    { "key": "1", "name": "Notepad", "enabled": true,
      "action": { "type": "launch", "path": "C:\\Windows\\notepad.exe", "args": [], "cwd": null } },
    { "key": "2", "action": { "type": "send_hotkey", "keys": "ctrl+shift+esc" } },
    { "key": "G", "action": { "type": "open_url", "url": "https://github.com" } }
  ]
}
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `shortcuts.enabled` | bool | `false` | Master switch. |
| `shortcuts.bindings` | array | `[]` | List of bindings. |
| `bindings[].key` | string | required | Hotkey: `hyper+<key>` or `<mods>+<key>` (see "Any hotkey"); a bare key name (table below) means Hyper + key. Case-insensitive. The key must not be a modifier or `capslock`. |
| `bindings[].name` | string | `""` | Label for the settings UI. Not used by the core. |
| `bindings[].enabled` | bool | `true` | Per-binding switch. |
| `bindings[].pass_through` | bool | `false` | The hotkey also reaches the app in front ("Also send to the app"). |
| `bindings[].action` | object | required | Tagged by `type`. |

Actions:

| `type` | Fields | Defaults |
|---|---|---|
| `launch` | `path` (string, required), `args` (array of strings **or** a single raw string), `cwd` (string or null) | `args: []`, `cwd: null` |
| `send_hotkey` | `keys` (string, required): one chord, e.g. `"ctrl+shift+esc"`, `"win+d"`, `"volume_mute"`, `"win"` | — |
| `open_url` | `url` (string, required) | — |

### Serde shape (`protocol/src/config.rs`)

```rust
// Phase 0 `Shortcuts` struct: only the type of `bindings` changes (was Vec<serde_json::Value>).
#[serde(default)] pub struct Shortcuts { pub enabled: bool, pub bindings: Vec<BindingEntry>,
                                         #[serde(flatten)] pub extra: serde_json::Map<String, Value> }

#[serde(untagged)]
pub enum BindingEntry { Valid(Binding), Invalid(serde_json::Value) }   // invalid entries are kept for round-trip, ignored at runtime

pub struct Binding { pub key: String, #[serde(default)] pub name: String,
                     #[serde(default = "yes")] pub enabled: bool, pub action: Action,
                     #[serde(flatten)] pub extra: serde_json::Map<String, Value> }

#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Launch { path: String, #[serde(default)] args: LaunchArgs, #[serde(default)] cwd: Option<String> },
    SendHotkey { keys: String },
    OpenUrl { url: String },
}
#[serde(untagged)] pub enum LaunchArgs { List(Vec<String>), Raw(String) }   // Default = List(vec![])
```

An unknown action `type` makes the entry `Invalid` (kept on save, skipped at runtime, logged once per load). Unknown fields at binding level survive via `extra`. Since Phase 5 the `Action` variants are newtype variants over `LaunchAction` / `SendHotkeyAction` / `OpenUrlAction`, each with its own `extra` map, so unknown fields *inside* `action` survive a save too (see [settings.md](settings.md)).

### Key names (`protocol/src/keys.rs`)

Case-insensitive, surrounding whitespace ignored. Each name maps to a `Key(u16)` (= Windows VK). Canonical name (first listed) is what `Key::name()` returns and what the settings recorder (Phase 5) writes.

| Names | Code |
|---|---|
| `a`…`z` | 0x41…0x5A |
| `0`…`9` (top row) | 0x30…0x39 |
| `f1`…`f24` | 0x70…0x87 |
| `esc` / `escape` | 0x1B |
| `tab`, `space`, `enter` / `return`, `backspace` | 0x09, 0x20, 0x0D, 0x08 |
| `insert` / `ins`, `delete` / `del`, `home`, `end`, `pageup` / `pgup`, `pagedown` / `pgdn` | 0x2D, 0x2E, 0x24, 0x23, 0x21, 0x22 |
| `left`, `up`, `right`, `down` | 0x25, 0x26, 0x27, 0x28 |
| `printscreen` / `prtsc`, `pause`, `scrolllock`, `numlock`, `capslock`, `apps` / `menu` | 0x2C, 0x13, 0x91, 0x90, 0x14, 0x5D |
| `num0`…`num9`, `num_multiply`, `num_add`, `num_subtract`, `num_decimal`, `num_divide` | 0x60…0x69, 0x6A, 0x6B, 0x6D, 0x6E, 0x6F |
| `oem_1` / `;`, `oem_plus` / `=`, `oem_comma` / `,`, `oem_minus` / `-`, `oem_period` / `.`, `oem_2` / `/`, `oem_3` / `` ` ``, `oem_4` / `[`, `oem_5` / `\`, `oem_6` / `]`, `oem_7` / `'`, `oem_8`, `oem_102` | 0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC0, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE2 |
| `volume_mute`, `volume_down`, `volume_up` | 0xAD, 0xAE, 0xAF |
| `media_next`, `media_prev`, `media_stop`, `media_play_pause` | 0xB0, 0xB1, 0xB2, 0xB3 |
| `ctrl` / `control`, `shift`, `alt`, `win` / `super` / `meta` (modifiers, for `send_hotkey` only) | send as LCtrl 0xA2, LShift 0xA0, LAlt 0xA4, LWin 0x5B |
| `lctrl`, `rctrl`, `lshift`, `rshift`, `lalt`, `ralt`, `lwin`, `rwin` | 0xA2, 0xA3, 0xA0, 0xA1, 0xA4, 0xA5, 0x5B, 0x5C |
| `vk:<hex>` e.g. `vk:0xE2` or `vk:E2` | raw code 0x01…0xFE |

Punctuation characters are aliases of the `oem_*` codes as they sit on the **US layout**. On other layouts (e.g. Spanish `ñ` = `oem_3`) use the `oem_*` name or `vk:`.

`send_hotkey` chord grammar: tokens separated by `+` (use `oem_plus` for the `+`/`=` key). All tokens except the last must be modifiers. If every token is a modifier (e.g. `"win"`), they are pressed and released in order. Max 4 modifiers + 1 key. Duplicates are an error.

```rust
pub fn parse_key(s: &str) -> Result<Key, KeyParseError>;
pub fn parse_chord(s: &str) -> Result<Chord, KeyParseError>;   // Chord { mods: [Key;4]/len, key: Option<Key> }
impl Key { pub fn name(self) -> String; pub fn is_modifier(self) -> bool; }
```

## Any hotkey (2026-11-01)

### Grammar (`protocol::parse_trigger`)

Tokens separated by `+`, case-insensitive, spaces around tokens ignored. All tokens but the last are modifiers (`ctrl`, `shift`, `alt`, `win` and their aliases/sided names) or `hyper`; the last is a key name from the table above.

| Text | Meaning |
|---|---|
| `"1"`, `"G"`, `"pageup"` | Hyper + key (Phase 2 format, still valid) |
| `"hyper+1"` | Hyper + 1 (what the settings app writes) |
| `"ctrl+t"`, `"win+e"`, `"ctrl+alt+k"`, `"ctrl+shift+f12"` | exactly those modifiers + key |

- Modifier sides are ignored: `rctrl+k` = `ctrl+k`, and either Ctrl key matches.
- Canonical form (`Trigger::to_config`): `hyper+<key>` or modifiers in the order `ctrl+shift+alt+win`, then the key's canonical name.
- Rejected (binding skipped with a warning, settings shows a message): a modifier or `capslock` as the key, a non-modifier before the last token, the same modifier twice (`ctrl+rctrl+k`), `hyper` with another modifier (`hyper+ctrl+k`; physical modifiers are ignored while Hyper is held), and the **reserved** combinations Ctrl+Alt+Del (any extra modifiers) and Win+L (Windows handles them before any hook).
- There is no way to bind a key without any modifier: a bare name means Hyper (compatibility), and a lone key would be swallowed everywhere.

### Behavior

- A normal hotkey fires on the trigger key's down when the modifiers physically held are **exactly** the bound set (Ctrl+T does not fire on Ctrl+Shift+T), Hyper not held. Once per press; repeats don't fire again.
- **Override by default**: the trigger's down, repeats and up are swallowed; the modifiers themselves always pass (they are not swallowed, so the app sees Ctrl down/up but no T). A swallowed normal hotkey injects the mask key right away, so releasing Win (Start menu), Alt (menu bar) or Ctrl+Shift (input language) afterwards does nothing.
- `pass_through: true`: the action runs **and** the key passes (no mask needed). Repeats pass without firing again. Works for Hyper hotkeys too (the app then gets the Hyper combo).
- While Hyper is held only Hyper hotkeys are looked up.
- `send_hotkey` from a normal hotkey: the user's modifiers are still down (Ctrl+T → `win+1` would arrive as Ctrl+Win+1). The batch is: mask, `Up` for every sided modifier the engine believes physically down, the chord, `Down` for the same modifiers, then mask again if Win or Alt is among them. All in one tagged `SendInput`.
- `launch` / `open_url` are unchanged (tagged empty mouse input + `AllowSetForegroundWindow`); the held modifiers simply go up later as the user releases them.
- Duplicates compare parsed triggers (`"1"` = `"hyper+1"`): the first enabled one wins; later ones are logged and flagged in the settings UI.

### Modifier state

The engine tracks the eight sided modifiers from the hook's events (never from Kuality's own tagged injections). To recover from an up the hook never saw (hook off while released, an elevated window in front): on a non-modifier key down while no Hyper modifier is injected and some modifier is believed down, the hook clears every bit `GetAsyncKeyState` reports up. The async state is only used to **clear**, never to set, so a wrong reading can only make a hotkey not fire. On hook install the state is copied once from `GetAsyncKeyState` (nothing of ours is injected then).

### Hook lifetime

The hook is installed when not paused and (`hyperkey.enabled` or the compiled table has a normal hotkey). With Hyperkey off the engine does not treat CapsLock as Hyper: CapsLock and the modifiers pass untouched. The main thread logs `Hyper shortcuts need Hyperkey` when an active binding is a Hyper hotkey and Hyperkey is off.

### Limitations

- Elevated (admin) windows in front: the hook does not see their keys (until "Run elevated", Phase 6).
- Another low-level hook installed after ours (AutoHotkey, RDP, VMs) sees keys first.
- Ctrl+Alt+Del and Win+L cannot be overridden.

## Implementation

### Code layout

```
crates/protocol/src/keys.rs                 key table, parse_key, parse_chord (portable, unit-tested)
crates/protocol/src/config.rs               Shortcuts (typed bindings), BindingEntry, Binding, Action, LaunchArgs
crates/core/src/shortcuts/mod.rs            compile(&Shortcuts) -> CompiledBindings (validation + warnings)
crates/core/src/hyperkey/engine.rs          + binding lookup and consumed-key tracking
crates/core/src/actions/mod.rs              ActionWorker: thread, channel, dispatch (portable)
crates/core/src/platform/windows/shell.rs   (exists from Phase 0) + launch/open_url via ShellExecuteExW, env expansion
crates/core/src/actions/quote.rs            Windows argument quoting (portable string logic, unit-tested)
crates/core/src/platform/windows/keyboard.rs + WM_APP_ACTION handling, send_hotkey injection
```

### Compilation (`shortcuts::compile`, main thread, on every config load)

```rust
pub struct CompiledBindings { pub generation: u64, pub entries: Vec<CompiledBinding> }
pub struct CompiledBinding { pub trigger: Trigger, pub pass_through: bool, pub source: usize, pub action: CompiledAction }
// lookups: find_hyper(key), find_normal(mods, key), has_normal(), has_hyper()
pub enum CompiledAction { Launch { path: String, params: String, cwd: Option<String> },
                          SendHotkey(Chord), OpenUrl(String) }
```

- Skip (with `warn`, never logging anything but the binding index and reason): `Invalid` entries, `enabled == false` (no warning), unknown key name or bad hotkey (see "Any hotkey"), modifier or `capslock` as trigger, reserved hotkey, empty `path`, chord parse error, URL without a valid scheme (`^[A-Za-z][A-Za-z0-9+.-]*:` and length ≤ 2048).
- Duplicate hotkey (same parsed trigger): the first enabled one wins, later ones skipped with `warn`.
- `LaunchArgs::List` is joined into one parameter string with Windows quoting (`CommandLineToArgvW` rules: quote if empty or containing space, tab, or `"`; double backslashes that precede a `"` or the closing quote; escape `"` as `\"`). `Raw` is passed verbatim.
- Environment variables (`%VAR%`) are **not** expanded here; expansion happens at execution time.
- If `shortcuts.enabled == false` the compiled list is empty.
- `generation` is incremented on every compile.
- Result is sent as `KeyboardCommand::Apply { hyperkey, shortcuts: Arc<CompiledBindings> }`.

### Engine changes

Engine gets `bindings: Arc<CompiledBindings>` (set on `Apply`) and `consumed: [u64; 4]` (bitset indexed by key code).

Inserted before Phase 1 rule 7 ("anything else → pass"):

- **Key down**, not a modifier/CapsLock:
  - if `consumed` has the key → swallow (auto-repeat of a consumed key).
  - else if `held` and the key is in `bindings` (linear scan; ≤ a few hundred entries, no allocation) → mark `consumed`, swallow, `verdict.action = Some(index)`.
- **Key up** with the key in `consumed` → clear it, swallow (regardless of `held`).

`reset()` and `force_release()` also clear `consumed`. Apply does not clear `consumed` (so in-flight ups are still swallowed).

### Dispatch from the hook callback

If `verdict.action = Some(i)`, the callback does `PostMessageW(hwnd, WM_APP_ACTION, i, generation)` (no allocation). The window proc (same thread, outside the callback):

1. If `generation` differs from the current bindings → drop (config changed in between).
2. `SendHotkey(chord)` → executed **right here on the keyboard thread** (it must coordinate with the injected Hyper modifiers, which only this thread owns):
   - If Hyper is held: release sequence for the injected modifiers (`MASK` down/up, then Up for injected LWIN, LALT, LSHIFT, LCTRL), mark them not injected.
   - Chord: Down for each modifier in order, Down/Up key (if any), Up for modifiers in reverse order. No mask (a chord of `"win"` is meant to open Start).
   - If Hyper is still held: Down LCTRL, LSHIFT, LALT, LWIN again, mark injected.
   - All in **one** `SendInput` call, tagged with `KUALITY_TAG`.
3. `Launch` / `OpenUrl` → clone the action and send it to the `ActionWorker` channel.

Log at `info`: `shortcut #<i> fired (<action type>)`. No key names in logs.

### Action worker (`actions/mod.rs` + `platform/windows/shell.rs`)

- Thread `kuality-actions`, spawned on the first action, lives until quit. Loop: `receiver.recv()`; exits when the channel closes (shutdown drops the sender, main joins with 1 s timeout).
- Thread init: `CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)`.
- Before each `ShellExecuteExW`: `AllowSetForegroundWindow(ASFW_ANY)` so the launched app can take focus.
- That grant ends at the next input, and releasing Hyper is input. So after a successful call the worker watches, for up to 3 s on a 50 ms thread timer (jobs keep running), for the first visible, unowned, non-tool top-level window of the started process (PID from `SEE_MASK_NOCLOSEPROCESS`), else of any process if it is new since the launch, and brings it forward (empty tagged mouse input + `SetForegroundWindow`) only while the foreground is still the window that was in front when the shortcut fired (noted when the job is queued). A newer launch replaces the watch.
- **launch**: expand `%VAR%` in `path`, `params`, `cwd` with `ExpandEnvironmentStringsW`. `cwd` default: parent directory of `path` if `path` is an absolute filesystem path, else `%USERPROFILE%`. `SHELLEXECUTEINFOW { fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS, lpVerb: "open", lpFile: path, lpParameters: params (null if empty), lpDirectory: cwd, nShow: SW_SHOWNORMAL }`. This supports `.exe`, `.lnk`, documents, folders and `shell:AppsFolder\<AUMID>` (Store apps).
- **open_url**: same call with `lpFile = url`, no parameters, no directory.
- Failure → `error` log with the binding's action type, the path/URL (config data, not keystrokes) and `GetLastError`. No dialogs.
- The worker never touches keyboard state.

## Edge cases

| Case | Handling |
|---|---|
| Bound key held down | Fires once; repeats and up swallowed via `consumed`. |
| Config reloaded between key down and dispatch | Generation mismatch → dropped. |
| Config reloaded while a bound key is held | `consumed` survives `Apply`; its up is still swallowed. |
| Key pressed before CapsLock, released while held | Not in `consumed` → passes through. |
| `send_hotkey` while Hyper held | Hyper modifiers released (with mask) before the chord, re-pressed after if still held. |
| `send_hotkey` of `ctrl+alt+del` | Windows ignores injected SAS; documented. |
| Foreground app elevated, core not | `SendInput` is blocked by UIPI (silent); launch/open_url still work. Documented. |
| Launched app does not come to the front | Windows foreground lock. Mitigations: empty tagged mouse input + `AllowSetForegroundWindow(ASFW_ANY)` before `ShellExecuteExW`, then the 3 s window watch (see above). An app that only reuses an existing window (new browser tab) is not raised by the watch. Report in manual test. |
| Slow launch (network path, big app) | Runs on the worker; hook/keyboard thread never waits. Actions are executed in order. |
| Explorer consumes Hyper+W/X/T/… | Bound keys are swallowed in the hook before Explorer sees them, so bound Office-letter shortcuts work. |
| Normal hotkey with Win / Alt (`win+e`, `alt+k`) | Trigger swallowed + mask key: Explorer does not open, Start / the menu bar do not open on release. |
| Modifier up missed (elevated window, hook off) | Cleared on the next key down from the async key state (only clears). |
| `ctrl+t` bound, user presses Ctrl+Shift+T | Not the bound set: passes. |
| Core elevated (Phase 6) | Launched apps would inherit elevation. Out of scope here; Phase 6 must launch via the shell (unelevated). |

## Out of scope

- Key sequences, text typing, chords with Hyper + extra modifiers, multiple chords per action, keys without any modifier (see "Any hotkey").
- Actions beyond the three (scripts, window management, extension actions — Phase 7).
- Running actions as admin / unelevated launch from an elevated core.
- Visual feedback (toast/OSD) when a shortcut fires or fails.
- ~~Shortcuts without Hyperkey~~ — normal hotkeys since 2026-11-01. A physical Hyper key from keyboard firmware (Ctrl+Shift+Alt+Win) can be bound as `ctrl+shift+alt+win+<key>`.

## Test plan

Automated:
- `parse_key`: every table row, case-insensitivity, `vk:` forms, rejects `""`, `vk:0x00`, `vk:0x1FF`, unknown names.
- `parse_chord`: `ctrl+shift+esc`, `win`, `win+d`, `ctrl+oem_plus`, spaces around tokens; rejects `a+b`, `ctrl+ctrl+a`, 5 modifiers, trailing `+`.
- `Key::name` round-trips through `parse_key` for every code in the table.
- Config: example JSON parses; unknown action type → `Invalid` and is re-serialized unchanged; `args` as string and as list; defaults when optional fields are missing.
- `compile`: duplicates (first wins), disabled entries, invalid key/url/path skipped, `enabled:false` → empty.
- Arg quoting: `a b` → `"a b"`, `x"y` → `"x\"y"`, `C:\dir\` with space → trailing backslash doubled, empty arg → `""`.
- Engine: held + bound key down → swallow + action; repeat → swallow no action; up → swallow; up after Caps released → swallow; unbound key → pass; not held → pass; `reset` clears consumed.
- Any hotkey: `parse_trigger` (bare key = Hyper, `hyper+…`, normal chords, sides, order, rejects, reserved); compile (normal triggers, duplicates across spellings, pass-through); engine (exact modifier match + mask, pass-through fires once and passes, normal hotkeys ignored while Hyper held, Hyperkey off leaves CapsLock alone, stale modifiers dropped / synced, `send_hotkey` from a normal chord releases and restores the user's modifiers); config defaults (`pass_through` false, `include_win` true, old files load).

Manual (Hyperkey + shortcuts enabled, the example config above):
1. Hyper+1 → Notepad opens (and takes focus). Holding Hyper+1 for 3 s opens exactly one Notepad.
2. Hyper+2 → Task Manager opens; keyviewer (from hyperkey.md) shows no stray keys; after releasing, no stuck modifiers.
3. Hyper+2, keep CapsLock held, press K → keyviewer receives Ctrl+Shift+Alt+Win+K (modifiers re-pressed).
4. Hyper+G → default browser opens github.com.
5. Hyper+K (unbound) → keyviewer receives the Hyper combo.
6. Bind `send_hotkey` `"win+d"` → desktop toggles; `"volume_mute"` → mutes.
7. Bind `launch` with `path: "%WINDIR%\\notepad.exe"`, `args: ["C:\\Temp\\a b.txt"]` → opens that file. Bind `shell:AppsFolder\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App` → Calculator opens.
8. Edit config while running (change key, disable binding, `shortcuts.enabled=false`) → applies without restart.
9. Invalid binding (`"key": "hyper"`) → warning in log, other bindings work.
10. Tray Pause → no shortcuts fire, CapsLock normal.
11. Bind an Office letter (`w`) → fires; Office/Word does not open.
12. `ctrl+alt+k` → its action runs, keyviewer sees Ctrl and Alt but no K. `win+e` → no Explorer window, no Start menu. `pass_through` on → the action runs and keyviewer sees the key. `ctrl+alt+j` → `send_hotkey` `shift+f11`: keyviewer sees F11 with only Shift down. With `hyperkey.enabled: false` the normal hotkeys still work and CapsLock toggles normally.

## Assumptions

1. Shortcuts require Hyperkey to be active; there is no way to trigger them without CapsLock in this phase. *(2026-11-01: Hyper hotkeys only; normal hotkeys work without Hyperkey.)*
2. Actions fire on key down, once per physical press, no auto-repeat.
3. Physical modifiers held with Hyper are ignored (Hyper+Shift+1 = Hyper+1). Normal hotkeys need the exact modifier set.
4. Key names map to Windows VK values; punctuation aliases follow the US layout; `oem_*` and `vk:` cover other layouts.
5. `send_hotkey` runs on the keyboard thread (not the action worker) because it must release/re-press the injected Hyper modifiers; launch/open_url run on the `kuality-actions` worker. ARCHITECTURE.md's thread list should note this when implementing.
6. `send_hotkey` accepts a single chord only; generic `ctrl/shift/alt/win` are injected as the left variants.
7. Each binding has optional `name` (UI label) and `enabled` (default true) fields in addition to `key` and `action`.
8. `launch.args` accepts a list (quoted and joined) or a raw string (verbatim); `cwd` is optional with a computed default; `%VAR%` is expanded at execution time.
9. `open_url` accepts any URI scheme (`https:`, `mailto:`, `ms-settings:`, `spotify:`…), validated only syntactically.
10. Invalid bindings are preserved in the file (untagged fallback) and skipped at runtime.
11. Duplicate hotkeys: first enabled binding wins.
12. No user-facing error UI; failures go to the log.
13. Launching unelevated from an elevated core is deferred to Phase 6.

## Dependencies

No new crates (`serde_json::Value` is already a Phase 0 dependency). Additional `windows` features in `crates/core`: `Win32_UI_Shell` (ShellExecuteExW), `Win32_System_Com` (CoInitializeEx), `Win32_System_Environment` (ExpandEnvironmentStringsW). `Win32_UI_WindowsAndMessaging` already covers `AllowSetForegroundWindow`.
