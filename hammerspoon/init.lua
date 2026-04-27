-- Screenshot bindings for ZMK MEDIA layer (F13-F18)
-- Matches Linux (Niri) screenshot shortcuts for same muscle memory

-- Cmd+Shift+Return = open email client (meli) in WezTerm.
-- Matches niri Mod+Shift+Return on Linux for cross-platform muscle memory.
-- Why Shift in the chord: bare Cmd+Return is heavily used app-internally
-- on macOS (Slack/Messages/Mail "send", form submit). Adding Shift sidesteps
-- those collisions while preserving the same mental model "<platform mod>
-- +Shift+Return = email" on both OSes.
-- Why email-wez and not email: today's settled architecture is bare meli
-- in WezTerm (Rust-coherent stack: WezTerm → meli). The `email` script
-- targets Ghostty via AppleScript and is kept around but not the canonical
-- launcher.
hs.hotkey.bind({"cmd", "shift"}, "return", function()
    hs.execute(os.getenv("HOME") .. "/.local/bin/email-wez &")
end)

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
