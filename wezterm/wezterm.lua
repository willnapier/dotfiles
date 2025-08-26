-- WezTerm configuration with advanced multiplexing and Solarized themes
local wezterm = require 'wezterm'
local act = wezterm.action
local mux = wezterm.mux

-- Helper function to get macOS appearance
local function get_appearance()
  if wezterm.gui then
    return wezterm.gui.get_appearance()
  end
  return 'Light'
end

-- Helper function to select theme based on appearance
local function scheme_for_appearance(appearance)
  if appearance:find 'Dark' then
    return 'Builtin Solarized Dark'
  else
    return 'Builtin Solarized Light'
  end
end

-- Helper function to detect screen size and return appropriate config
local function get_screen_config()
  -- Get display dimensions
  for _, screen in ipairs(wezterm.gui and wezterm.gui.screens() or {}) do
    local width = screen.width
    local height = screen.height
    
    -- Large screen (32" 6K Dell or similar)
    if width >= 5120 then  -- 6K width
      return {
        default_layout = "four_pane",
        pane_strategy = "aggressive",
        initial_cols = 200,
        initial_rows = 60
      }
    -- Medium/Large screen  
    elseif width >= 2560 then  -- 4K or large
      return {
        default_layout = "dual_pane", 
        pane_strategy = "moderate",
        initial_cols = 140,
        initial_rows = 40
      }
    -- Laptop screen (15" MacBook Air)
    else
      return {
        default_layout = "single_pane",
        pane_strategy = "minimal", 
        initial_cols = 100,
        initial_rows = 30
      }
    end
  end
  
  -- Fallback for laptop
  return {
    default_layout = "single_pane",
    pane_strategy = "minimal",
    initial_cols = 100, 
    initial_rows = 30
  }
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
config.colors = {
  selection_fg = 'none',  -- Keep original text color
  selection_bg = 'rgba(88, 110, 117, 0.25)',  -- Darker, more subtle highlight for better contrast
  -- Let the theme handle cursor and ANSI colors
}

-- Font configuration
config.font = wezterm.font("JetBrainsMono Nerd Font")
config.font_size = 14
config.line_height = 1.2

-- Tab bar disabled (using Aerospace for window management instead)
config.enable_tab_bar = false

-- Window configuration
config.window_decorations = "RESIZE"
config.window_padding = {
  left = 10,
  right = 10,
  top = 10,
  bottom = 5,
}

-- Initial window size (in character cells)
-- This sets a reasonable default that works on both large and small screens
config.initial_cols = 120  -- Width in columns
config.initial_rows = 35   -- Height in rows

-- Cursor
config.default_cursor_style = 'BlinkingBar'
config.cursor_blink_rate = 600
config.cursor_blink_ease_in = 'Constant'
config.cursor_blink_ease_out = 'Constant'

-- Scrollback
config.scrollback_lines = 10000

-- Clipboard integration - basic settings
config.enable_csi_u_key_encoding = true
config.use_ime = true

-- Image protocol support for tools like Yazi
config.enable_kitty_graphics = true

-- Advertise Kitty Graphics Protocol support
config.term = "wezterm"

-- Default shell - Smart Zellij startup with session management (fixed terminal check)
-- This ensures Zellij is available for wiki link integration when appropriate
-- Falls back to Nushell if already inside Zellij or no proper terminal
config.default_prog = { '/Users/williamnapier/.local/bin/smart-zellij-start.sh' }

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

-- Key bindings
config.keys = {
  -- macOS word navigation
  {
    key = 'LeftArrow',
    mods = 'OPT',
    action = act.SendKey { key = 'b', mods = 'ALT' },
  },
  {
    key = 'RightArrow',
    mods = 'OPT',
    action = act.SendKey { key = 'f', mods = 'ALT' },
  },
  -- Delete word backward
  {
    key = 'Backspace',
    mods = 'OPT',
    action = act.SendKey { key = 'w', mods = 'CTRL' },
  },
  -- Delete to line start (standard macOS behavior)
  {
    key = 'Backspace',
    mods = 'CMD',
    action = act.SendString '\x15',  -- Ctrl+U (kill line backward)
  },
  -- Jump to line start/end with Cmd+Arrow
  {
    key = 'LeftArrow',
    mods = 'CMD',
    action = act.SendKey { key = 'a', mods = 'CTRL' },
  },
  {
    key = 'RightArrow',
    mods = 'CMD',
    action = act.SendKey { key = 'e', mods = 'CTRL' },
  },
  
  -- Direct pane commands (no leader needed!)
  {
    key = 'h',
    mods = 'CMD|SHIFT',
    action = act.SplitVertical { domain = 'CurrentPaneDomain' },
  },
  {
    key = 'v',
    mods = 'CMD|SHIFT',
    action = act.SplitHorizontal { domain = 'CurrentPaneDomain' },
  },
  {
    key = 'z',
    mods = 'CMD|SHIFT',
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
    mods = 'CMD|SHIFT',
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
    mods = 'CMD|SHIFT',
    action = act.RotatePanes 'Clockwise',
  },
  
  -- Colemak-DH pane navigation (neio for hjkl) - matching Helix/Yazi/Zellij
  {
    key = 'n',
    mods = 'CMD|SHIFT', 
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'e',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Down', 
  },
  {
    key = 'i',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'o',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Right',
  },
  
  -- Keep arrow key navigation as backup
  {
    key = 'LeftArrow',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Left',
  },
  {
    key = 'RightArrow',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Right',
  },
  {
    key = 'UpArrow',
    mods = 'CMD|SHIFT',
    action = act.ActivatePaneDirection 'Up',
  },
  {
    key = 'DownArrow',
    mods = 'CMD|SHIFT',
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
          args = { '/opt/homebrew/bin/nu', '-c', 'daily-note' },
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
  -- Mouse selection automatically copies on release
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
  
  -- Set window to a reasonable size and center it
  -- This ensures consistent sizing when switching between monitors
  window:gui_window():set_position(100, 50)
  
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