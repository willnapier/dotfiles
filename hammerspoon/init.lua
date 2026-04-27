-- Enable IPC so `hs -c "..."` can talk to the running Hammerspoon
-- process. Lets remote/programmatic reloads work via `hs -c hs.reload()`
-- instead of needing to click the menu-bar icon. No behaviour change for
-- the user — purely an automation surface.
require("hs.ipc")

-- Screenshot bindings for ZMK MEDIA layer (F13-F18)
-- Matches Linux (Niri) screenshot shortcuts for same muscle memory

-- Cmd+Shift+Return → email-wez moved to Karabiner (2026-04-27).
-- Karabiner's IOHIDSystem-level event tap is more reliable than
-- Hammerspoon's user-space hotkey hooks, which wedged 4 times in a
-- single session of intensive programmatic use. The rule lives in
-- ~/dotfiles/karabiner/karabiner.json under
-- complex_modifications.rules → "Cmd+Shift+Return → email-wez ...".
-- Hammerspoon retains the screenshot bindings (F13–F18) where its
-- ergonomics are still the right fit.

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
