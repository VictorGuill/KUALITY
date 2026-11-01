# Hyperkey (Phase 1)

## Goal

While CapsLock is held, the system behaves as if Ctrl+Shift+Alt+Win are held. CapsLock itself never reaches applications. Tapping CapsLock alone has no visible effect. The feature is off by default, hot-reloads, can be paused from the tray, and never leaves modifiers stuck.

## Behavior (user-visible)

- `hyperkey.enabled = false` (default): no hook is installed; CapsLock is a normal CapsLock.
- `hyperkey.enabled = true` and not paused ("active"):
  - CapsLock down → apps see LCtrl, LShift, LAlt, LWin go down. CapsLock up → they go up. Holding CapsLock + T → apps receive Ctrl+Shift+Alt+Win+T.
  - Tapping CapsLock alone: no Start menu, no menu bar, no Office hub, no input-language switch, CapsLock light/toggle does not change.
  - Real CapsLock is unavailable. If CapsLock is toggled ON when Hyperkey becomes active (or found ON later), Kuality turns it OFF.
  - Physical Ctrl/Shift/Alt/Win pressed while CapsLock is held add nothing (they are already part of Hyper) and are ignored.
  - Office letters: Hyper+W/X/P/O/T/D/N/L/Y open nothing (the letter is dropped unless a shortcut is bound to it; see Office key below).
- The Phase 0 tray menu gains a checkable **Pause** item right after Settings: `Settings`, `Pause`, separator, `Quit`. Pause suspends **all keyboard features** (Hyperkey and, from Phase 2, Shortcuts) by uninstalling the hook. Pause is runtime-only (not saved; Kuality always starts unpaused). The tray tooltip becomes `Kuality (paused)` while paused. Pause does not affect widgets.
- Office key: while `hyperkey.enabled` is true and the core is running, Windows' Office-key launcher is disabled; it is restored when Hyperkey is disabled in config or Kuality quits cleanly.

## Config

```json
"hyperkey": { "enabled": false, "include_win": true }
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `hyperkey.enabled` | bool | `false` | Turn the CapsLock → Hyper remap on. |
| `hyperkey.include_win` | bool | **`true`** | *(2026-11-01)* Hyper is Ctrl+Shift+Alt+**Win**. Off: Ctrl+Shift+Alt. |

*(2026-11-01)* `include_win` off: Hyper presses and releases only LCtrl, LShift, LAlt (rule 2 / release sequence skip LWin). The Office letters are not dropped (rule 7) and the Office-key registry override is not applied (restored if it was): both exist only because Explorer owns Ctrl+Shift+Alt+**Win** (+letter), which Hyper no longer produces. On some layouts (e.g. Spanish) Ctrl+Alt acts as AltGr, so Hyper+some keys may type a character; the settings app says so.

No other keys. Missing section/field → default. Changes apply on hot-reload within one reload cycle.

## Implementation

### Code layout

```
crates/protocol/src/config.rs          Hyperkey { enabled, extra }: unchanged from Phase 0 (skeleton.md)
crates/protocol/src/keys.rs            Key(u16) + name table (see shortcuts.md; Phase 1 only needs the constants)
crates/core/src/hyperkey/engine.rs     portable state machine (no Win32 types), unit-tested
crates/core/src/hyperkey/mod.rs        KeyboardController: public API used by main (apply config, pause, shutdown)
crates/core/src/platform/windows/keyboard.rs    hook thread, hook proc, hidden window, notifications
crates/core/src/platform/windows/input.rs       SendInput helpers (Synth → INPUT), extended-key table
crates/core/src/platform/windows/office_key.rs  registry override apply/restore
```

`protocol::keys::Key(pub u16)` is Kuality's key code. Its numeric values **equal Windows virtual-key codes** (documented as such); a future port maps native codes to them. Constants needed now: `CAPSLOCK=0x14`, `LCTRL=0xA2`, `RCTRL=0xA3`, `LSHIFT=0xA0`, `RSHIFT=0xA1`, `LALT=0xA4`, `RALT=0xA5`, `LWIN=0x5B`, `RWIN=0x5C`, generic `CTRL=0x11`, `SHIFT=0x10`, `ALT=0x12`, `MASK=0xE8` (unassigned VK used as the mask key).

### Portable engine (`hyperkey/engine.rs`)

```rust
pub struct KeyEvent { pub key: Key, pub down: bool }        // only non-Kuality-injected events reach the engine
pub enum Synth { Down(Key), Up(Key) }                       // events to inject
pub struct Verdict { pub swallow: bool, pub synth: SynthBuf, pub action: Option<u16> }
// SynthBuf: fixed array [Synth; 32] + len (no heap allocation). `action` is used by Phase 2.

pub struct Engine {
    held: bool,                 // Hyper is active (CapsLock physically down)
    injected: ModSet,           // which of LCTRL/LSHIFT/LALT/LWIN we have injected down
    physical: ModSet8,          // physical state of the 8 sided modifiers (L/R Ctrl, Shift, Alt, Win)
    // Phase 2 adds: consumed: [u64; 4] (bitset by key code), bindings lookup
}
impl Engine {
    pub fn on_event(&mut self, ev: KeyEvent, caps_toggled_on: bool) -> Verdict;
    pub fn force_release(&mut self) -> SynthBuf;   // used by disable/pause/lock/suspend/shutdown
    pub fn reset(&mut self);                        // forget all state without injecting (after unlock/resume)
    pub fn modifiers_down(&self) -> bool;
}
```

Rules (exhaustive):

1. Every event updates `physical` if the key is one of the 8 sided modifiers (generic `CTRL/SHIFT/ALT` never arrive in LL hooks; ignore them).
2. `CAPSLOCK` down, `held == false` → `held = true`; synth = (if `caps_toggled_on`: `Down(CAPSLOCK), Up(CAPSLOCK)`) then `Down(LCTRL), Down(LSHIFT), Down(LALT), Down(LWIN)` (no `LWIN` when `include_win` is off); `injected` = those; swallow. *(2026-11-01: rules 2–5 apply only while `hyperkey.enabled`; with Hyperkey off the hook may still run for normal hotkeys, and CapsLock then passes.)*
3. `CAPSLOCK` down, `held == true` (auto-repeat) → swallow, no synth.
4. `CAPSLOCK` up, `held == true` → synth = release sequence (below); `held = false`; swallow.
5. `CAPSLOCK` up, `held == false` (we missed the down, e.g. hook installed while CapsLock was down) → swallow, no synth.
6. Any of the 8 sided modifiers, down or up, while `held` → swallow, no synth (only `physical` is updated).
7. Anything else → pass (`swallow = false`). Phase 2 inserts its binding check here, followed by the Office-letter drop: a down of one of `OFFICE_LETTERS` (W, X, P, O, T, D, N, L, Y) while `held`, `include_win` on and not bound → swallow and mark consumed (its repeats and up are swallowed too, as for bound keys).

Release sequence (`force_release` returns the same when `held`; empty otherwise):
`Down(MASK), Up(MASK)`, then `Up` for each key in `injected` in order LWIN, LALT, LSHIFT, LCTRL, **skipping any whose sided key is physically down** in `physical` (so a Shift the user is still holding stays down). Then `injected = empty`, `held = false`.

The mask is always sent (cheap and it covers Start menu, menu bar, Office hub and Alt+Shift / Ctrl+Shift input-language switching).

### Hook thread (`platform/windows/keyboard.rs`)

- One thread `kuality-keyboard`, spawned at core startup, alive until quit. Priority `THREAD_PRIORITY_HIGHEST`.
- It owns: a hidden **top-level** window (class `KualityKeyboard`, `WS_POPUP`, never shown, `WS_EX_TOOLWINDOW`) so it receives broadcasts; the `Engine` in a `thread_local! RefCell`; the `HHOOK`; one `HWINEVENTHOOK`.
- Message loop: plain `GetMessageW` / `TranslateMessage` / `DispatchMessageW`. No modal loops on this thread.
- Commands from other threads: `std::sync::mpsc::Sender<KeyboardCommand>` + `PostMessageW(hwnd, WM_APP_COMMAND, 0, 0)` to wake; the window proc drains the channel. This "channel + wake message" pattern is the standard for all Kuality worker threads.

```rust
pub enum KeyboardCommand {
    Apply { hyperkey: Hyperkey /* Phase 2 adds: shortcuts: Arc<CompiledBindings> */ },
    SetPaused(bool),
    Shutdown,
}
```

- `active = config.enabled && !paused`. On every command recompute `active`:
  - becomes active → install hook (`SetWindowsHookExW(WH_KEYBOARD_LL, proc, GetModuleHandleW(None), 0)`), then if CapsLock is toggled on, inject `CAPSLOCK` down/up (tagged) to turn it off.
  - becomes inactive → `force_release` (inject result), `UnhookWindowsHookEx`.
  - stays active → nothing (but `Apply` always reinstalls the hook: unhook + hook, after `force_release`; cheap, and recovers a hook Windows silently removed).
- `Shutdown` → `force_release`, unhook, unregister notifications, `DestroyWindow`, `PostQuitMessage(0)`. Main joins the thread (1 s timeout).

### Hook callback (strict rules)

```
extern "system" fn proc(code, wparam, lparam) -> LRESULT
```

- If `code < 0` → `CallNextHookEx`.
- Read `KBDLLHOOKSTRUCT`. If `flags & LLKHF_INJECTED != 0 && dwExtraInfo == KUALITY_TAG` → `CallNextHookEx` immediately (our own events; prevents loops). `KUALITY_TAG = 0x4B55_414C` ("KUAL"), defined once in `platform/windows/input.rs`. Events injected by other software are processed like physical ones.
- `down = wparam is WM_KEYDOWN | WM_SYSKEYDOWN`; up for `WM_KEYUP | WM_SYSKEYUP`.
- `caps_toggled_on = GetKeyState(VK_CAPITAL) & 1 != 0` (only evaluated for CapsLock down while not held).
- Borrow the engine, call `on_event`, **drop the borrow**, then if `synth` is non-empty call `SendInput` once with all events.
- Return `1` if `swallow`, else `CallNextHookEx`.
- Whole body inside `std::panic::catch_unwind`; on panic: emergency release (below) and `CallNextHookEx`.
- Forbidden inside the callback: heap allocation, locks, logging, registry/file I/O, `SendMessage`, anything that can block. Allowed: the engine call, `GetKeyState`, `GetAsyncKeyState` (8 sided modifiers, only to clear missed modifier ups for normal hotkeys, see shortcuts.md), one `SendInput` (≤ 32 INPUTs), one `PostMessageW` to our own window.
- Logging: the callback may `PostMessageW(hwnd, WM_APP_TRACE, code, 0)` for state transitions (Hyper down, Hyper up, forced release); the window proc logs them at `debug`. **Never log key codes or typed keys** (privacy).

### SendInput (`platform/windows/input.rs`)

- Each `Synth` → `INPUT_KEYBOARD` with `wVk = key`, `wScan = MapVirtualKeyW(key, MAPVK_VK_TO_VSC_EX) & 0xFF`, `dwFlags = KEYEVENTF_KEYUP` for up, plus `KEYEVENTF_EXTENDEDKEY` if the key is in the extended set or the mapped scan code has the `0xE0` prefix, `dwExtraInfo = KUALITY_TAG`, `time = 0`. `MASK` uses `wScan = 0`.
- Extended set: RCTRL, RALT, LWIN, RWIN, APPS, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, arrows, NUMLOCK, PRINTSCREEN, numpad DIVIDE, and all browser/media/volume/launch keys.
- After any successful injection, update a global `static MODIFIERS_DOWN: AtomicBool` from `engine.modifiers_down()` (used by emergency release).

### Lock screen, sleep, desktop switches

The hidden window registers:
- `WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)`.
- `SetWinEventHook(EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH, None, cb, 0, 0, WINEVENT_OUTOFCONTEXT)` (fires on lock, Ctrl+Alt+Del, UAC secure desktop).
- `WM_POWERBROADCAST` arrives as a broadcast to the top-level hidden window.

| Event | Action |
|---|---|
| `WTS_SESSION_LOCK`, `WTS_CONSOLE_DISCONNECT`, `WTS_REMOTE_DISCONNECT`, `PBT_APMSUSPEND`, `EVENT_SYSTEM_DESKTOPSWITCH` | `force_release` + inject (may fail on the secure desktop; ignore), then `reset()` |
| `WTS_SESSION_UNLOCK`, `WTS_CONSOLE_CONNECT`, `WTS_REMOTE_CONNECT`, `PBT_APMRESUMEAUTOMATIC` | `reset()`; inject `Up` for any of LCTRL/LSHIFT/LALT/LWIN that `GetAsyncKeyState` reports down (clears leftovers); if active: reinstall hook and turn CapsLock toggle off if on |

### Crash recovery

- At startup the core installs `std::panic::set_hook` (chaining the Phase 0 logging panic hook) and `SetUnhandledExceptionFilter`. Both call `input::emergency_release()`: if `MODIFIERS_DOWN` is set, one `SendInput` with `MASK` down/up and Up for LWIN, LALT, LSHIFT, LCTRL. It allocates nothing and never panics.
- If the process is killed (`taskkill /f`), Windows removes the hook; modifiers injected down may stay logically down until the user presses them. Documented limitation (nothing can run on a hard kill).

### Office key (`platform/windows/office_key.rs`)

Explorer registers `Ctrl+Shift+Alt+Win` alone and with W/X/T/O/P/D/L/N/Y as Office hotkeys. Two mechanisms block them:

- **Office letters** (engine rule 7): while Hyper is held, unbound Office letters are dropped in the hook, so Explorer never sees them. Observed on Windows 11 IoT Enterprise LTSC, build 26300 (2026-11-01): with only the `ms-officeapp` override, Explorer still opened `https://go.microsoft.com/fwlink/?linkid=…` pages in the default browser (T Teams 2044782, W 2044666, X 2044684, P 2044769, O 2044778, N 2044773, Y 2044904, L LinkedIn 2044786; D opened nothing). `https` cannot be overridden, and apps never receive these combos anyway (Explorer takes the key down). Hyper+L opened LinkedIn and did not lock the PC.
- **Office key alone**: launched through the `ms-officeapp` protocol. Override it in HKCU:

- *(2026-11-01)* Wanted while `hyperkey.enabled && hyperkey.include_win`; everything below that says "enabled" means this.
- **Apply** (idempotent): if `HKCU\Software\Kuality\OfficeKey` value `Applied` (DWORD) is 1 → do nothing. Otherwise record backup under `HKCU\Software\Kuality\OfficeKey`: `PrevExisted` (DWORD 0/1: whether `HKCU\Software\Classes\ms-officeapp` existed) and `PrevCommand` (REG_SZ, the previous default value of `...\ms-officeapp\Shell\Open\Command`, only if it existed). Then create `HKCU\Software\Classes\ms-officeapp\Shell\Open\Command` with default value (REG_SZ) `rundll32`, set `Applied = 1`, call `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, null, null)`.
- **Restore**: if `Applied != 1` → nothing. If `PrevExisted == 0` → `RegDeleteTreeW(HKCU\Software\Classes\ms-officeapp)`. Else if `PrevCommand` was recorded → write it back as the default value; else delete only the default value we wrote. Delete `HKCU\Software\Kuality\OfficeKey`, `SHChangeNotify`.
- Runs on the **main thread** (registry I/O must not delay the hook thread):
  - at startup after the first config load: `enabled` → Apply, else → Restore (cleans up after a crash);
  - on reload when `enabled` changes;
  - on clean quit → Restore.
- Pause does not touch it.
- Failures are logged (`warn`) and never stop Hyperkey.

### Integration with main (`app.rs` from skeleton.md)

- `main.rs` creates `KeyboardController::start()` at startup (spawns the thread, returns a handle holding the `Sender` and `HWND`).
- After the first config load, and in `app.rs` whenever a `WM_APP_CONFIG` reload delivers a new `Config`: `controller.apply(&config.hyperkey)` and Office key reconcile. (Apply on every reload; the thread reinstalling the hook is harmless.)
- Tray Pause click: toggle `paused`, `controller.set_paused(paused)`, update check mark + tooltip.
- Quit: `controller.shutdown()` (join), Office key Restore, then the rest of Phase 0 shutdown.

## Edge cases

| Case | Handling |
|---|---|
| CapsLock auto-repeat | Swallowed, nothing injected (rule 3). |
| Physical modifier held before/while Hyper | Swallowed while held; not released at Hyper up if still physically down (rule 6 + release sequence). |
| Hook installed while CapsLock is down | Its up is swallowed with no synth (rule 5). |
| CapsLock up missed (focus moved to an elevated window, hook dropped by Windows) | Self-heals on next CapsLock press/release (rule 3 then 4). Elevated targets also drop our `SendInput` (UIPI). Documented; fixed by "Run elevated" (Phase 6). |
| CapsLock toggle got turned on (elevated window had focus, another tool) | Turned off on the next Hyper press, on activation, unlock and resume. |
| Lock screen, Ctrl+Alt+Del, UAC, sleep | Forced release + reset (table above). |
| Hook silently removed by Windows (`LowLevelHooksTimeout`) | Callback stays far below the timeout; hook is reinstalled on every `Apply`, unlock and resume. |
| Explorer restart | No effect on the hook. |
| Disable/pause while CapsLock held | `force_release` before unhooking. |
| Another LL-hook app (AutoHotkey, RDP client, VM) installed after us | It sees keys first; whatever it swallows never reaches us. Documented. |
| Fullscreen games | Remap stays active; user can Pause from the tray. |
| Hyper+L | Office letter: dropped unless bound (before, it opened LinkedIn; it did not lock the PC in tests with injected input). |
| Physical Ctrl+Shift+Alt+Win+Office letter (no CapsLock) | Not Hyper: reaches Explorer and opens the Office page. Limitation. |
| Another tool registered an Office-letter combo as its own hotkey | Dropped too while Hyper is held (no setting); bind the letter in Kuality instead. |

## Out of scope

- Real CapsLock via double-tap, tap actions, remapping any key other than CapsLock, choosing which modifiers Hyper sends (beyond `include_win`).
- Hooking keys that go to elevated windows when the core is not elevated (Phase 6 "Run elevated").
- Disabling Explorer's own registration of the Office hotkeys (requires restarting Explorer).

## Test plan

Automated (`cargo test`, pure engine, no Win32):
- Caps down → 4 modifier downs + swallow; repeats → swallow, no synth; Caps up → mask down/up + 4 ups in order LWIN, LALT, LSHIFT, LCTRL.
- Caps down with `caps_toggled_on` → CapsLock down/up first.
- Physical LShift held before Caps, released during Hyper, re-pressed and still down at Caps up → LShift not released.
- Physical modifier events while held are swallowed.
- Caps up without prior down → swallow, no synth.
- `force_release` when held / not held; `reset` clears state.
- Other keys pass through untouched.
- `office_key` backup/restore logic behind a small `trait RegistryOps` with an in-memory fake.

Manual (Windows 11), with `hyperkey.enabled = true`. Add a dev-only example `crates/core/examples/keyviewer.rs`: a small window that prints every `WM_KEYDOWN/UP/SYSKEYDOWN/UP` with the state of Ctrl/Shift/Alt/Win (`GetKeyState`).
1. CapsLock+T in keyviewer → Ctrl+Shift+Alt+Win all down, then T. Repeat with K. Release → all up.
2. Tap CapsLock 20 times: no Start menu, no menu bar in Notepad, no Office/office.com, CapsLock light never changes, input language does not change (configure Alt+Shift switching to test).
3. Hold CapsLock 5 s: keyviewer shows no repeated modifier spam.
4. Turn CapsLock on with Hyperkey disabled, enable it in config.json → CapsLock turns off.
5. Toggle `enabled` in config.json while app runs → takes effect without restart. Tray Pause → CapsLock is normal; unpause → Hyper again.
6. Hold CapsLock, press Win+L / Ctrl+Alt+Del, unlock → no stuck modifiers (type in Notepad: plain letters).
7. Sleep/resume while idle, and while holding CapsLock → no stuck modifiers, Hyper still works.
8. Tray Quit while holding CapsLock → no stuck modifiers; Office key registry restored.
9. Office key: with Hyperkey enabled, Hyper alone and Hyper+W/X/T/O/P/D/L/N/Y open nothing (no new browser tab); a bound Office letter fires its shortcut; Hyper+K still reaches keyviewer. Disable → registry restored to original state (check with `reg query`).
10. Idle: Task Manager shows 0% CPU for the core with Hyperkey enabled; no measurable lag typing.
11. Debug build: log shows Hyper transitions but no key codes.

## Assumptions

1. Integration follows skeleton.md: `app.rs` on the main thread receives each reloaded `Config` (`WM_APP_CONFIG`) and forwards sections to subsystems; the `Hyperkey` config type is reused unchanged.
2. The Phase 0 tray menu becomes `Settings`, `Pause` (checkable), separator, `Quit`; Pause is runtime-only and pauses keyboard features only (not widgets).
3. Key codes are a `Key(u16)` newtype in `protocol` whose values equal Windows VK codes.
4. Hyper is injected as the **left** variants (LCtrl, LShift, LAlt, LWin).
5. The mask key is the unassigned VK `0xE8` and is sent on every Hyper release.
6. Injected-event tag is `dwExtraInfo = 0x4B55414C`; events injected by other programs are treated like physical ones.
7. Physical Ctrl/Shift/Alt/Win are swallowed while Hyper is held.
8. The hook thread exists for the whole process life; the hook itself is installed only while active.
9. Office key override is tied to `hyperkey.enabled` (not to Pause), restored on clean quit, reconciled at startup; backup lives in `HKCU\Software\Kuality\OfficeKey`.
10. *(Revised 2026-11-01.)* The registry override alone does not stop Hyper+letter: Explorer opens the Office web pages directly. The hook therefore drops unbound Office letters while Hyper is held (rule 7); bound ones run their shortcut. Explorer's hotkey registration itself is left alone.
11. Crash recovery uses a panic hook + unhandled exception filter; hard kills are a documented limitation.
12. No new crates. Only additional `windows` crate features (below).

## Dependencies

No new crates. `windows` features to enable in `crates/core`:
`Win32_Foundation`, `Win32_UI_WindowsAndMessaging` (hook, window, messages), `Win32_UI_Input_KeyboardAndMouse` (SendInput, GetKeyState, MapVirtualKeyW), `Win32_System_LibraryLoader` (GetModuleHandleW), `Win32_System_Threading` (thread priority), `Win32_System_RemoteDesktop` (WTSRegisterSessionNotification), `Win32_UI_Accessibility` (SetWinEventHook), `Win32_System_Power` (PBT_* constants), `Win32_System_Registry`, `Win32_UI_Shell` (SHChangeNotify), `Win32_System_Diagnostics_Debug` (SetUnhandledExceptionFilter).
