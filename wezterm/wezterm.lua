-- ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/wezterm/
-- ⚠️  NEVER EDIT ~/.config/wezterm/wezterm.lua - IT'S A SYMLINK TO THIS FILE
-- WezTerm configuration with advanced multiplexing and Solarized themes
local wezterm = require 'wezterm'
local act = wezterm.action
local mux = wezterm.mux

-- Helper function to get system appearance (cross-platform)
local function get_appearance()
  local is_macos = wezterm.target_triple:find("apple") ~= nil
  local is_linux = wezterm.target_triple:find("linux") ~= nil

  if is_macos then
    -- macOS: Use native appearance detection
    if wezterm.gui then
      return wezterm.gui.get_appearance()
    end
    return 'Light'
  elseif is_linux then
    -- Linux: Check GNOME color-scheme setting
    local success, stdout, stderr = wezterm.run_child_process({
      'gsettings', 'get', 'org.gnome.desktop.interface', 'color-scheme'
    })

    if success and stdout then
      -- Output is like: 'prefer-dark' or 'prefer-light' or 'default'
      if stdout:match('dark') then
        return 'Dark'
      else
        return 'Light'
      end
    end

    -- Fallback to dark if gsettings unavailable
    return 'Dark'
  else
    -- Unknown platform: default to Light
    return 'Light'
  end
end

-- Helper function to select theme based on appearance
local function scheme_for_appearance(appearance)
  if appearance:find 'Dark' then
    return 'Builtin Solarized Dark'
  else
    return 'Builtin Solarized Light'
  end
end

-- Cross-platform key modifier mapping
local function get_platform_modifiers()
  local is_macos = wezterm.target_triple:find("apple") ~= nil
  local is_linux = wezterm.target_triple:find("linux") ~= nil

  if is_macos then
    return {
      word_mod = 'OPT',      -- Option/Alt on macOS
      line_mod = 'CMD',      -- Command on macOS
      primary_mod = 'CMD',   -- Primary modifier (Cmd on macOS)
      font_mod = 'CMD',      -- Font controls (Cmd on macOS)
    }
  else -- Linux and other platforms
    return {
      word_mod = 'ALT',      -- Alt on Linux
      line_mod = 'CTRL',     -- Ctrl on Linux
      primary_mod = 'CTRL',  -- Primary modifier (Ctrl on Linux)
      font_mod = 'ALT',      -- Font controls (Alt on Linux, won't conflict with Zellij)
    }
  end
end

local mods = get_platform_modifiers()

-- Cross-platform Nushell path detection
local function get_nushell_path()
  local is_macos = wezterm.target_triple:find("apple") ~= nil

  if is_macos then
    return '/opt/homebrew/bin/nu'
  else -- Linux and other platforms
    return 'nu'  -- Use PATH resolution for Linux
  end
end

local nushell_path = get_nushell_path()

-- Helper function to get appropriate window size based on screen resolution
local function get_window_size()
  -- Use system_profiler to safely detect screen size
  -- This avoids GUI issues during WezTerm startup
  local handle = io.popen("system_profiler SPDisplaysDataType | grep 'Resolution:' | head -1")
  local res_line = handle:read("*a")
  handle:close()
  
  -- Extract width and height from "Resolution: 1920 x 1242"
  local width, height = res_line:match("Resolution: (%d+) x (%d+)")
  width = tonumber(width) or 1920
  height = tonumber(height) or 1200
  
  -- Calculate maximum columns and rows based on font metrics
  -- JetBrainsMono 14pt: ~8px wide × 17px tall (with line_height 1.2)
  local char_width = 6.5
  local char_height = 16
  local padding_x = -120  -- Minimal window padding left + right
  local padding_y = -40  -- Just menu bar space (reduce from 80)
  
  local max_cols = math.floor((width - padding_x) / char_width)
  local max_rows = math.floor((height - padding_y) / char_height)
  
  -- Return appropriate window size based on screen width
  if width >= 6000 then
    -- 32" Dell 6K - use calculated maximum
    return { initial_cols = math.min(max_cols, 700), initial_rows = math.min(max_rows, 180) }
  elseif width >= 2560 then
    -- 27" Display - use calculated maximum
    return { initial_cols = math.min(max_cols, 300), initial_rows = math.min(max_rows, 75) }
  else
    -- Laptop - use calculated maximum for full screen usage
    return { initial_cols = max_cols, initial_rows = max_rows }
  end
end

-- Configuration
local config = {}

-- Use the config builder for better error messages
if wezterm.config_builder then
  config = wezterm.config_builder()
end

-- Appearance and theming - dynamically detect current appearance
config.color_scheme = scheme_for_appearance(get_appearance())
config.window_background_opacity = 0.98
config.macos_window_background_blur = 20

-- Custom selection colors for better readability in Solarized themes
-- Explicitly set background to ensure it matches zellij's solarized-dark theme
config.colors = {
  background = '#002b36',  -- Solarized Dark base03 (teal) - matches zellij bg 0 43 54
  foreground = '#839496',  -- Solarized Dark base0
  selection_fg = 'none',  -- Keep original text color
  selection_bg = 'rgba(88, 110, 117, 0.25)',  -- Darker, more subtle highlight for better contrast
}

-- Font configuration
config.font = wezterm.font("JetBrainsMono Nerd Font")
config.font_size = 13
config.line_height = 1.2

-- Tab bar disabled (using Aerospace for window management instead)
config.enable_tab_bar = false

-- Window configuration
-- NONE on Linux/Wayland (niri handles window management), RESIZE on macOS
local is_linux = wezterm.target_triple:find("linux") ~= nil
if is_linux then
  config.window_decorations = "NONE"
else
  config.window_decorations = "RESIZE"
end
config.window_padding = {
  left = 0,
  right = 0,
  top = 0,
  bottom = 0,
}

-- Window frame colors (title bar) - Solarized Dark
config.window_frame = {
  inactive_titlebar_bg = '#002b36',  -- Solarized base03
  active_titlebar_bg = '#002b36',    -- Same teal for both
  inactive_titlebar_fg = '#586e75',  -- Solarized base01
  active_titlebar_fg = '#93a1a1',    -- Solarized base1
}

-- Let WezTerm handle sizing with maximize() instead of manual calculations
-- config.initial_cols and config.initial_rows removed to let maximize() work

-- Cursor
config.default_cursor_style = 'BlinkingBar'
config.cursor_blink_rate = 600
config.cursor_blink_ease_in = 'Constant'
config.cursor_blink_ease_out = 'Constant'

-- Scrollback
config.scrollback_lines = 10000

-- Clipboard integration - basic settings
config.enable_csi_u_key_encoding = false  -- Disabled: was intercepting Mod+Alt combos before Niri could see them
config.use_ime = true

-- Alt key handling - Enable right alt for composed characters like # symbol
config.send_composed_key_when_left_alt_is_pressed = false
config.send_composed_key_when_right_alt_is_pressed = true

-- Image protocol support for tools like Yazi
config.enable_kitty_graphics = true

-- Advertise Kitty Graphics Protocol support
config.term = "wezterm"

-- Default shell - Nushell directly (Zellij auto-start disabled for debugging)
-- Use manual `zellij` command when you want multiplexing
config.default_prog = { nushell_path }

-- Multiplexing domains
config.ssh_domains = {
  -- Example SSH domain configuration
  -- {
  --   name = 'myserver',
  --   remote_address = 'server.example.com',
  --   username = 'myuser',
  -- },
}

-- Leader key for multiplexing (similar to tmux)
config.leader = { key = 'b', mods = 'CTRL', timeout_milliseconds = 1000 }

-- Cross-platform key bindings
config.keys = {
  -- Disable Cmd+W on macOS to prevent accidental tab/window closing
  -- Especially important in Claude Code where accidental close loses session
  {
    key = 'w',
    mods = 'SUPER',
    action = act.DisableDefaultAssignment,
  },

  -- Disable Ctrl+Z to prevent accidental process suspension
  -- Especially important in Claude Code where accidental suspension breaks flow
  {
    key = 'z',
    mods = 'CTRL',
    action = act.DisableDefaultAssignment,
  },

  -- Pass Ctrl +/- through to Zellij (instead of WezTerm font controls)
  -- Send the actual key combo to the terminal application
  {
    key = '-',
    mods = 'CTRL',
    action = act.SendKey { key = '-', mods = 'CTRL' },
  },
  {
    key = '=',
    mods = 'CTRL',
    action = act.SendKey { key = '=', mods = 'CTRL' },
  },
  {
    key = '+',
    mods = 'CTRL',
    action = act.SendKey { key = '+', mods = 'CTRL' },
  },
  -- Pass Ctrl Shift +/= through to Zellij as well (for Ctrl + which is Ctrl Shift =)
  {
    key = '=',
    mods = 'SHIFT|CTRL',
    action = act.SendKey { key = '=', mods = 'CTRL' },  -- Send as Ctrl = (Zellij sees it as increase)
  },
  {
    key = '+',
    mods = 'SHIFT|CTRL',
    action = act.SendKey { key = '=', mods = 'CTRL' },  -- Send as Ctrl = (same as above)
  },
  {
    key = '-',
    mods = 'SHIFT|CTRL',
    action = act.DisableDefaultAssignment,  -- Ctrl Shift - not needed
  },
  -- Disable SUPER (Cmd) font controls - we'll reassign below
  {
    key = '-',
    mods = 'SUPER',
    action = act.DisableDefaultAssignment,
  },
  {
    key = '=',
    mods = 'SUPER',
    action = act.DisableDefaultAssignment,
  },
  {
    key = '+',
    mods = 'SUPER',
    action = act.DisableDefaultAssignment,
  },

  -- Font size controls (cross-platform: Cmd on macOS, Alt on Linux)
  {
    key = '-',
    mods = mods.font_mod,
    action = act.DecreaseFontSize,
  },
  {
    key = '=',
    mods = mods.font_mod,
    action = act.IncreaseFontSize,
  },
  {
    key = '+',
    mods = mods.font_mod,
    action = act.IncreaseFontSize,
  },
  {
    key = '0',
    mods = mods.font_mod,
    action = act.ResetFontSize,
  },

  -- Cross-platform word navigation (Alt on all platforms, Option on macOS)
  {
    key = 'LeftArrow',
    mods = mods.word_mod,
    action = act.SendKey { key = 'b', mods = 'ALT' },
  },
  {
    key = 'RightArrow',
    mods = mods.word_mod,
    action = act.SendKey { key = 'f', mods = 'ALT' },
  },

  -- Cross-platform line start/end navigation (Cmd on macOS, Ctrl on Linux)
  {
    key = 'LeftArrow',
    mods = mods.line_mod,
    action = act.SendKey { key = 'a', mods = 'CTRL' },
  },
  {
    key = 'RightArrow',
    mods = mods.line_mod,
    action = act.SendKey { key = 'e', mods = 'CTRL' },
  },

  -- Cross-platform deletion shortcuts
  {
    key = 'Backspace',
    mods = mods.word_mod,
    action = act.SendKey { key = 'w', mods = 'CTRL' },  -- Delete word backward
  },
  {
    key = 'Backspace',
    mods = mods.line_mod,
    action = act.SendString '\x15',  -- Delete to line start (Ctrl+U)
  },
  {
    key = 'Delete',
    mods = mods.word_mod,
    action = act.SendKey { key = 'd', mods = 'ALT' },  -- Delete word forward
  },
  {
    key = 'Delete',
    mods = mods.line_mod,
    action = act.SendString '\x0b',  -- Delete to line end (Ctrl+K)
  },

  -- Cross-platform selection with Shift+arrows (word selection)
  {
    key = 'LeftArrow',
    mods = 'SHIFT|' .. mods.word_mod,
    action = act.SendString '\x1b[1;6D',  -- Shift+Alt+Left
  },
  {
    key = 'RightArrow',
    mods = 'SHIFT|' .. mods.word_mod,
    action = act.SendString '\x1b[1;6C',  -- Shift+Alt+Right
  },

  -- Cross-platform selection with Shift+arrows (line selection)
  {
    key = 'LeftArrow',
    mods = 'SHIFT|' .. mods.line_mod,
    action = act.SendString '\x1b[1;2H',  -- Shift+Home
  },
  {
    key = 'RightArrow',
    mods = 'SHIFT|' .. mods.line_mod,
    action = act.SendString '\x1b[1;2F',  -- Shift+End
  },
  
  -- Direct pane commands (no leader needed!) - Cross-platform
  {
    key = 'h',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.SplitVertical { domain = 'CurrentPaneDomain' },
  },
  {
    key = 'v',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.SplitHorizontal { domain = 'CurrentPaneDomain' },
  },
  {
    key = 'z',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.TogglePaneZoomState,
  },
  
  -- Multiplexing: Panes (leader backup)
  {
    key = '"',
    mods = 'LEADER|SHIFT',
    action = act.SplitVertical { domain = 'CurrentPaneDomain' },
  },
  {
    key = '%',
    mods = 'LEADER|SHIFT',
    action = act.SplitHorizontal { domain = 'CurrentPaneDomain' },
  },
  {
    key = 'z',
    mods = 'LEADER',
    action = act.TogglePaneZoomState,
  },
  {
    key = 'x',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.CloseCurrentPane { confirm = true },
  },
  {
    key = 'x',
    mods = 'LEADER',
    action = act.CloseCurrentPane { confirm = true },
  },
  
  -- Rotate panes (swap positions) - no leader needed!
  {
    key = 'r',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.RotatePanes 'Clockwise',
  },
  
  -- Colemak-DH pane navigation (neio for hjkl) - matching Helix/Yazi/Zellij
  {
    key = 'n',
    mods = mods.primary_mod .. '|SHIFT', 
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'e',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Down', 
  },
  {
    key = 'i',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'o',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Right',
  },
  
  -- Keep arrow key navigation as backup
  {
    key = 'LeftArrow',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'RightArrow',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Right',
  },
  {
    key = 'UpArrow',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'DownArrow',
    mods = mods.primary_mod .. '|SHIFT',
    action = act.ActivatePaneDirection 'Down',
  },
  
  -- Colemak-DH pane navigation with Leader (neio for hjkl)
  {
    key = 'n',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'e', 
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Down',
  },
  {
    key = 'i',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'o',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Right',
  },
  
  -- Keep Leader+Arrow navigation as backup
  {
    key = 'LeftArrow',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'RightArrow',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Right',
  },
  {
    key = 'UpArrow',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'DownArrow',
    mods = 'LEADER',
    action = act.ActivatePaneDirection 'Down',
  },
  
  -- Resize panes (Miryoku layout: N=left, E=down, I=up, O=right)
  {
    key = 'N',
    mods = 'LEADER|SHIFT',
    action = act.AdjustPaneSize { 'Left', 5 },
  },
  {
    key = 'E',
    mods = 'LEADER|SHIFT',
    action = act.AdjustPaneSize { 'Down', 5 },
  },
  {
    key = 'I',
    mods = 'LEADER|SHIFT',
    action = act.AdjustPaneSize { 'Up', 5 },
  },
  {
    key = 'O',
    mods = 'LEADER|SHIFT',
    action = act.AdjustPaneSize { 'Right', 5 },
  },
  
  
  -- Workspaces (similar to tmux sessions)
  {
    key = 's',
    mods = 'LEADER',
    action = act.ShowLauncherArgs { flags = 'FUZZY|WORKSPACES' },
  },
  
  -- Copy mode (vim-like)
  {
    key = '[',
    mods = 'LEADER',
    action = act.ActivateCopyMode,
  },
  
  -- Paste
  {
    key = ']',
    mods = 'LEADER',
    action = act.PasteFrom 'Clipboard',
  },
  
  -- Standard macOS clipboard shortcuts
  {
    key = 'c',
    mods = mods.primary_mod,
    action = act.CopyTo 'Clipboard',
  },
  {
    key = 'v',
    mods = mods.primary_mod,
    action = act.PasteFrom 'Clipboard',
  },
  -- Extra clipboard fallbacks (ensure both Cmd/Ctrl + Shift combos work)
  {
    key = 'v',
    mods = 'CTRL|SHIFT',
    action = act.PasteFrom 'Clipboard',
  },
  {
    key = 'v',
    mods = 'SUPER',
    action = act.PasteFrom 'Clipboard',
  },
  {
    key = 'v',
    mods = 'SUPER|SHIFT',
    action = act.PasteFrom 'Clipboard',
  },
  
  -- Command palette
  {
    key = 'P',
    mods = 'LEADER|SHIFT',
    action = act.ActivateCommandPalette,
  },
  
  -- Quick split with preset commands
  {
    key = 'g',
    mods = 'LEADER',
    action = wezterm.action_callback(function(window, pane)
      window:perform_action(
        act.SplitHorizontal {
          args = { 'lazygit' },
        },
        pane
      )
    end),
  },
  
  -- Quick note-taking split
  {
    key = 'd',
    mods = 'LEADER',
    action = wezterm.action_callback(function(window, pane)
      window:perform_action(
        act.SplitVertical {
          args = { nushell_path, '-c', 'daily-note' },
        },
        pane
      )
    end),
  },
  
  -- Theme switching
  {
    key = 'T',
    mods = 'LEADER|SHIFT',
    action = wezterm.action_callback(function(window, pane)
      local overrides = window:get_config_overrides() or {}
      if overrides.color_scheme == 'Builtin Solarized Dark' then
        overrides.color_scheme = 'Builtin Solarized Light'
      else
        overrides.color_scheme = 'Builtin Solarized Dark'
      end
      window:set_config_overrides(overrides)
    end),
  },
  
  -- Search scrollback
  {
    key = '/',
    mods = 'LEADER',
    action = act.Search { CaseSensitiveString = '' },
  },
  
  -- Quad layout: Claude Code (top-left), Nushell (bottom-left), nvim Telekasten (top-right), yazi (bottom-right)
  {
    key = 'q',
    mods = 'LEADER',
    action = wezterm.action_callback(function(window, pane)
      -- Start Claude Code in current pane
      pane:send_text('claude code\n')
      
      -- Create the splits step by step with delays
      wezterm.time.call_after(1, function()
        -- Split horizontally (left|right)
        window:perform_action(act.SplitHorizontal { domain = 'CurrentPaneDomain' }, pane)
        
        wezterm.time.call_after(0.5, function()
          -- Go to right pane and start nvim
          window:perform_action(act.ActivatePaneDirection 'Right', pane)
          local right_pane = window:active_pane()
          right_pane:send_text('nvim +Telekasten\n')
          
          wezterm.time.call_after(0.5, function()
            -- Go back to left and split vertically (top|bottom)
            window:perform_action(act.ActivatePaneDirection 'Left', pane)
            window:perform_action(act.SplitVertical { domain = 'CurrentPaneDomain' }, pane)
            
            wezterm.time.call_after(0.5, function()
              -- Go to right pane and split vertically
              window:perform_action(act.ActivatePaneDirection 'Right', pane)
              window:perform_action(act.SplitVertical { domain = 'CurrentPaneDomain' }, pane)
              
              wezterm.time.call_after(0.5, function()
                -- Go to bottom-right and start yazi
                window:perform_action(act.ActivatePaneDirection 'Down', pane)
                local yazi_pane = window:active_pane()
                yazi_pane:send_text('yazi\n')
                
                -- Return to top-left
                window:perform_action(act.ActivatePaneDirection 'Up', pane)
                window:perform_action(act.ActivatePaneDirection 'Left', pane)
              end)
            end)
          end)
        end)
      end)
    end),
  },
}

-- Mouse bindings - explicit clipboard handling
config.mouse_bindings = {
  -- Mouse selection automatically copies
  {
    event = { Up = { streak = 1, button = 'Left' } },
    mods = 'NONE',
    action = act.CompleteSelection 'ClipboardAndPrimarySelection',
  },
  
  -- Double-click selects word and copies
  {
    event = { Up = { streak = 2, button = 'Left' } },
    mods = 'NONE',
    action = act.CompleteSelection 'ClipboardAndPrimarySelection',
  },
  
  -- Triple-click selects line and copies
  {
    event = { Up = { streak = 3, button = 'Left' } },
    mods = 'NONE',
    action = act.CompleteSelection 'ClipboardAndPrimarySelection',
  },
  
  -- Right click pastes from clipboard
  {
    event = { Down = { streak = 1, button = 'Right' } },
    mods = 'NONE', 
    action = act.PasteFrom 'Clipboard',
  },
  
  -- CTRL + Click opens links
  {
    event = { Up = { streak = 1, button = 'Left' } },
    mods = 'CTRL',
    action = act.OpenLinkAtMouseCursor,
  },
}

-- Copy mode key bindings (vim-like)
config.key_tables = {
  copy_mode = {
    { key = 'Tab', mods = 'NONE', action = act.CopyMode 'MoveForwardWord' },
    { key = 'Tab', mods = 'SHIFT', action = act.CopyMode 'MoveBackwardWord' },
    { key = 'Enter', mods = 'NONE', action = act.CopyMode 'MoveToStartOfNextLine' },
    { key = 'Escape', mods = 'NONE', action = act.CopyMode 'Close' },
    { key = 'Space', mods = 'NONE', action = act.CopyMode{ SetSelectionMode = 'Cell' } },
    { key = '$', mods = 'NONE', action = act.CopyMode 'MoveToEndOfLineContent' },
    { key = '0', mods = 'NONE', action = act.CopyMode 'MoveToStartOfLine' },
    { key = 'G', mods = 'NONE', action = act.CopyMode 'MoveToScrollbackBottom' },
    { key = 'G', mods = 'SHIFT', action = act.CopyMode 'MoveToScrollbackBottom' },
    { key = 'g', mods = 'NONE', action = act.CopyMode 'MoveToScrollbackTop' },
    { key = 'h', mods = 'NONE', action = act.CopyMode 'MoveLeft' },
    { key = 'j', mods = 'NONE', action = act.CopyMode 'MoveDown' },
    { key = 'k', mods = 'NONE', action = act.CopyMode 'MoveUp' },
    { key = 'l', mods = 'NONE', action = act.CopyMode 'MoveRight' },
    { key = 'v', mods = 'NONE', action = act.CopyMode{ SetSelectionMode = 'Cell' } },
    { key = 'V', mods = 'NONE', action = act.CopyMode{ SetSelectionMode = 'Line' } },
    { key = 'V', mods = 'SHIFT', action = act.CopyMode{ SetSelectionMode = 'Line' } },
    { key = 'v', mods = 'CTRL', action = act.CopyMode{ SetSelectionMode = 'Block' } },
    { key = 'w', mods = 'NONE', action = act.CopyMode 'MoveForwardWord' },
    { key = 'y', mods = 'NONE', action = act.Multiple{ { CopyTo = 'ClipboardAndPrimarySelection' }, { CopyMode = 'Close' } } },
  },
  
  search_mode = {
    { key = 'Enter', mods = 'NONE', action = act.CopyMode 'PriorMatch' },
    { key = 'Escape', mods = 'NONE', action = act.CopyMode 'Close' },
    { key = 'n', mods = 'CTRL', action = act.CopyMode 'NextMatch' },
    { key = 'p', mods = 'CTRL', action = act.CopyMode 'PriorMatch' },
    { key = 'r', mods = 'CTRL', action = act.CopyMode 'CycleMatchType' },
    { key = 'u', mods = 'CTRL', action = act.CopyMode 'ClearPattern' },
  },
}

-- Status update callbacks
wezterm.on('update-right-status', function(window, pane)
  local cells = {}
  
  -- Show workspace name
  local stat = window:active_workspace()
  table.insert(cells, stat)
  
  -- Show current time
  local date = wezterm.strftime '%H:%M'
  table.insert(cells, date)
  
  -- Color based on theme
  local color_scheme = window:effective_config().color_scheme
  local bg = '#002b36' -- Solarized dark base03
  local fg = '#93a1a1' -- Solarized dark base1
  
  if color_scheme:find('light') then
    bg = '#fdf6e3' -- Solarized light base3
    fg = '#586e75' -- Solarized light base01
  end
  
  local text = ' ' .. table.concat(cells, ' | ') .. ' '
  window:set_right_status(wezterm.format {
    { Background = { Color = bg } },
    { Foreground = { Color = fg } },
    { Text = text },
  })
end)

-- Startup workspaces
wezterm.on('gui-startup', function()
  -- Default workspace
  local tab, pane, window = mux.spawn_window {
    workspace = 'main',
    cwd = wezterm.home_dir,
  }
  
  -- Get the GUI window
  local gui_window = window:gui_window()

  -- Platform-conditional window sizing:
  -- macOS: maximize() since no tiling WM
  -- Linux: let Niri handle via window-rules
  local is_linux = wezterm.target_triple:find("linux") ~= nil
  if not is_linux and gui_window then
    gui_window:maximize()
  end
  
  -- Auto-split for laptop layout (two panes side by side)
  wezterm.time.call_after(0.5, function()
    -- Get screen width to determine if this is laptop layout
    local handle = io.popen("system_profiler SPDisplaysDataType | grep 'Resolution:' | head -1")
    local res_line = handle:read("*a")
    handle:close()
    
    local width = res_line:match("Resolution: (%d+) x")
    width = tonumber(width) or 1920
    
    -- Only auto-split for laptop screens (< 2560px width)
    if width < 2560 then
      -- Create vertical split (two panes side by side)
      window:perform_action(wezterm.action.SplitHorizontal { domain = 'CurrentPaneDomain' }, pane)
    end
  end)
  
  -- Create a notes workspace (commented out to avoid second window)
  -- local notes_tab, notes_pane, notes_window = mux.spawn_window {
  --   workspace = 'notes',
  --   cwd = wezterm.home_dir .. '/Obsidian.nosync/Forge',
  -- }
  
  -- Switch back to main workspace
  -- mux.set_active_workspace 'main'
end)

-- Simple approach: detect appearance on each config reload
wezterm.on('window-focus-changed', function(window, pane)
  local overrides = window:get_config_overrides() or {}
  local current_appearance = get_appearance()
  local current_scheme = scheme_for_appearance(current_appearance)
  
  if overrides.color_scheme ~= current_scheme then
    overrides.color_scheme = current_scheme
    window:set_config_overrides(overrides)
  end
end)

return config
