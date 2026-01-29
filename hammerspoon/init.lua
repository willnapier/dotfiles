-- Screenshot bindings for ZMK MEDIA layer (F13-F18)
-- Matches Linux (Niri) screenshot shortcuts for same muscle memory

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
