Kuality (portable)
==================

Version {version}.

Lightweight Windows 11 utility: taskbar widgets, CapsLock Hyperkey and keyboard shortcuts.
Everything is disabled until you turn it on.

Start
-----
Double-click kuality.exe. A tray icon appears; click it to open the settings window.
Right-click the tray icon for Pause and Quit.

Portable mode
-------------
The empty file "portable" next to kuality.exe keeps config.json and the logs folder in this
folder. Delete it to use %APPDATA%\Kuality instead.

Start with Windows
------------------
Settings window > General > Startup. "Run as administrator" makes Hyperkey and shortcuts work in
apps running as administrator too (Windows asks for permission when you change it).
If you move this folder, turn the option off and on again.

Remove
------
Turn "Start with Windows" off first (or run: kuality.exe --startup off), quit Kuality from the
tray icon, then delete this folder.

License: MIT (LICENSE.txt).
