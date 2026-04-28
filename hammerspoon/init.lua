-- Enable IPC so `hs -c "..."` can talk to the running Hammerspoon
-- process. Lets remote/programmatic reloads work via `hs -c hs.reload()`
-- instead of needing to click the menu-bar icon. No behaviour change for
-- the user — purely an automation surface.
require("hs.ipc")

-- Screenshot bindings for ZMK MEDIA layer (F13-F18)
-- Matches Linux (Niri) screenshot shortcuts for same muscle memory

-- ARCHITECTURE: this file owns "global keyboard chord → shell command".
-- Karabiner owns "per-device key remapping" (built-in keyboard Colemak,
-- Piantor "disable built-in when connected" gate). ZMK firmware owns
-- the Piantor's layout. Three layers, three responsibilities, no overlap.
--
-- Why not put global hotkeys in Karabiner: complex_modifications only
-- fire for devices with Modify events ON. The Piantor needs Modify
-- events OFF (otherwise Karabiner's virtual-keyboard injection breaks
-- macOS's Cmd+` window cycling — discovered 2026-04-27). So Karabiner
-- can't host Piantor-originating global chords. Hammerspoon receives
-- the chord at the OS hotkey layer, regardless of source device.

local screenshotDir = os.getenv("HOME") .. "/Pictures/Screenshots/"
hs.execute("mkdir -p '" .. screenshotDir .. "'")

local function timestamp()
    return os.date("%Y-%m-%d_%H-%M-%S")
end

-- F13 = Region to file (MEDIA + J)
hs.hotkey.bind({}, "F13", function()
    hs.execute("screencapture -i '" .. screenshotDir .. timestamp() .. "_region.png'")
end)

-- F14 = Full screen to file (MEDIA + L)
hs.hotkey.bind({}, "F14", function()
    hs.execute("screencapture '" .. screenshotDir .. timestamp() .. "_full.png'")
end)

-- F15 = Window to file (MEDIA + U)
hs.hotkey.bind({}, "F15", function()
    hs.execute("screencapture -w '" .. screenshotDir .. timestamp() .. "_window.png'")
end)

-- F16 = Region to clipboard (MEDIA + Y)
hs.hotkey.bind({}, "F16", function()
    hs.execute("screencapture -ic")
end)

-- F17 = Full screen to clipboard (MEDIA + ')
hs.hotkey.bind({}, "F17", function()
    hs.execute("screencapture -c")
    hs.alert.show("📸 Full screen → clipboard", 0.5)
end)

-- F18 = Window to clipboard (MEDIA + ?)
hs.hotkey.bind({}, "F18", function()
    hs.execute("screencapture -wc")
end)

-- Cmd+Shift+Return → open meli in WezTerm.
-- Anti-wedge: hs.task spawns asynchronously, so the callback returns
-- without blocking Hammerspoon's main thread on `wezterm start`. Earlier
-- wedging (4 incidents in a 2026-04-26 intensive session) was observed
-- with synchronous patterns under load; fire-and-forget keeps the main
-- thread responsive. The screenshot bindings above use hs.execute because
-- screencapture returns near-instantly; if any of them ever wedge under
-- load, port to hs.task on the same pattern.
hs.hotkey.bind({"cmd", "shift"}, "return", function()
    hs.task.new("/Users/williamnapier/.local/bin/email", function() end):start()
end)
