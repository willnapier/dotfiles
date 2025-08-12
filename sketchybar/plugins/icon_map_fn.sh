#!/usr/bin/env bash

function __icon_map() {
  local app="$1"
  
  case "$app" in
    # System Apps
    "Finder" | "访达")
      icon_result="󰀶"
      ;;
    "System Preferences" | "System Settings")
      icon_result="󰒓"
      ;;
    "App Store")
      icon_result="󰒋"
      ;;
    "Activity Monitor")
      icon_result="󰍛"
      ;;
    "Archive Utility")
      icon_result="󰀼"
      ;;
    "Calculator")
      icon_result="󰃬"
      ;;
    "Calendar" | "日历" | "iCal")
      icon_result="󰃭"
      ;;
    "Contacts")
      icon_result="󰛋"
      ;;
    "Dictionary")
      icon_result="󰈐"
      ;;
    "Disk Utility")
      icon_result="󰋊"
      ;;
    "Font Book")
      icon_result="󰛖"
      ;;
    "Image Capture")
      icon_result="󰄄"
      ;;
    "Keychain Access")
      icon_result="󰌾"
      ;;
    "Mail" | "邮件")
      icon_result="󰊫"
      ;;
    "Messages" | "信息" | "Nachrichten")
      icon_result="󰍡"
      ;;
    "Migration Assistant")
      icon_result="󰤆"
      ;;
    "Notes" | "备忘录")
      icon_result="󰎞"
      ;;
    "Photo Booth")
      icon_result="󰄄"
      ;;
    "Photos")
      icon_result="󰋣"
      ;;
    "Preview" | "预览")
      icon_result="󰈙"
      ;;
    "QuickTime Player")
      icon_result="󰐊"
      ;;
    "Reminders" | "提醒事项")
      icon_result="󰄲"
      ;;
    "Screenshot")
      icon_result="󰹑"
      ;;
    "Stickies")
      icon_result="󰐃"
      ;;
    "TextEdit")
      icon_result="󰈙"
      ;;
    "Time Machine")
      icon_result="󰁯"
      ;;
    "Console")
      icon_result="󰆍"
      ;;
    
    # Browsers
    "Safari" | "Safari浏览器" | "Safari Technology Preview")
      icon_result="󰀹"
      ;;
    "Google Chrome" | "Google Chrome Canary" | "Chromium")
      icon_result="󰊯"
      ;;
    "Firefox" | "Firefox Developer Edition" | "Firefox Nightly")
      icon_result="󰈙"
      ;;
    "Microsoft Edge")
      icon_result="󰇩"
      ;;
    "Opera")
      icon_result="󰆍"
      ;;
    "Brave Browser")
      icon_result="󰇧"
      ;;
    "Vivaldi")
      icon_result="󰖟"
      ;;
    "Arc")
      icon_result="󰣨"
      ;;
    "DuckDuckGo")
      icon_result="󰈙"
      ;;
    "Tor Browser")
      icon_result="󰖟"
      ;;
    "Min")
      icon_result="󰈹"
      ;;
    "Orion" | "Orion RC")
      icon_result="󰣆"
      ;;
    "qutebrowser")
      icon_result="󰖟"
      ;;
    "LibreWolf")
      icon_result="󰈙"
      ;;
    
    # Developer Tools
    "Terminal" | "终端")
      icon_result="󰆍"
      ;;
    "iTerm" | "iTerm2")
      icon_result="󰆍"
      ;;
    "Alacritty")
      icon_result="󰆍"
      ;;
    "kitty")
      icon_result="󰄛"
      ;;
    "Warp")
      icon_result="󰞷"
      ;;
    "Hyper")
      icon_result="󰆍"
      ;;
    "WezTerm")
      icon_result="󰆍"
      ;;
    "Visual Studio Code" | "Code" | "Code - Insiders")
      icon_result="󰨞"
      ;;
    "VSCodium")
      icon_result="󰨞"
      ;;
    "Sublime Text")
      icon_result="󰀶"
      ;;
    "Sublime Merge")
      icon_result="󰊢"
      ;;
    "Atom")
      icon_result="󰀶"
      ;;
    "TextMate")
      icon_result="󰈙"
      ;;
    "Nova")
      icon_result="󰈮"
      ;;
    "BBEdit")
      icon_result="󰈙"
      ;;
    "CotEditor")
      icon_result="󰈙"
      ;;
    "Xcode")
      icon_result="󰀵"
      ;;
    "Instruments")
      icon_result="󰍛"
      ;;
    "Simulator")
      icon_result="󰀴"
      ;;
    "Android Studio")
      icon_result="󰀴"
      ;;
    "IntelliJ IDEA")
      icon_result="󰈙"
      ;;
    "WebStorm")
      icon_result="󰈙"
      ;;
    "PhpStorm")
      icon_result="󰈙"
      ;;
    "PyCharm")
      icon_result="󰈙"
      ;;
    "RubyMine")
      icon_result="󰴭"
      ;;
    "GoLand")
      icon_result="󰟓"
      ;;
    "DataGrip")
      icon_result="󰆼"
      ;;
    "Rider" | "JetBrains Rider")
      icon_result="󰈙"
      ;;
    "Tower")
      icon_result="󰊢"
      ;;
    "SourceTree")
      icon_result="󰊢"
      ;;
    "GitHub Desktop")
      icon_result="󰊤"
      ;;
    "Fork")
      icon_result="󰊢"
      ;;
    "GitKraken")
      icon_result="󰊢"
      ;;
    "Kaleidoscope")
      icon_result="󰊢"
      ;;
    "TablePlus")
      icon_result="󰆼"
      ;;
    "Sequel Pro" | "Sequel Ace")
      icon_result="󰆼"
      ;;
    "Postico")
      icon_result="󰆼"
      ;;
    "Postman")
      icon_result="󰛶"
      ;;
    "Insomnia")
      icon_result="󰛶"
      ;;
    "Paw")
      icon_result="󰛶"
      ;;
    "Charles")
      icon_result="󰒍"
      ;;
    "Proxyman")
      icon_result="󰒍"
      ;;
    "Docker" | "Docker Desktop")
      icon_result="󰡨"
      ;;
    "VirtualBox")
      icon_result="󰖳"
      ;;
    "VMware Fusion")
      icon_result="󰖳"
      ;;
    "Parallels Desktop")
      icon_result="󰖳"
      ;;
    "UTM")
      icon_result="󰖳"
      ;;
    "Zed")
      icon_result="󰈮"
      ;;
    "Vim" | "MacVim" | "VimR" | "Neovide")
      icon_result="󰕷"
      ;;
    "Emacs")
      icon_result="󰘧"
      ;;
    "Kakoune")
      icon_result="󰈙"
      ;;
    
    # Communication
    "Slack")
      icon_result="󰒱"
      ;;
    "Discord" | "Discord Canary" | "Discord PTB")
      icon_result="󰙯"
      ;;
    "Zoom" | "zoom.us")
      icon_result="󰍎"
      ;;
    "Microsoft Teams")
      icon_result="󰊻"
      ;;
    "Skype")
      icon_result="󰒯"
      ;;
    "WhatsApp")
      icon_result="󰖣"
      ;;
    "Telegram")
      icon_result="󰔁"
      ;;
    "Signal")
      icon_result="󰈙"
      ;;
    "FaceTime" | "FaceTime 通话")
      icon_result="󰍫"
      ;;
    "Webex" | "Webex Meetings")
      icon_result="󰍎"
      ;;
    "Google Meet")
      icon_result="󰍎"
      ;;
    "微信" | "WeChat")
      icon_result="󰘑"
      ;;
    "카카오톡" | "KakaoTalk")
      icon_result="󰍡"
      ;;
    "DingTalk" | "钉钉" | "阿里钉")
      icon_result="󰍡"
      ;;
    "Element")
      icon_result="󰍡"
      ;;
    "Zulip")
      icon_result="󰍡"
      ;;
    "TeamSpeak 3")
      icon_result="󰖣"
      ;;
    "Caprine")
      icon_result="󰈎"
      ;;
    "Android Messages")
      icon_result="󰍡"
      ;;
    
    # Productivity
    "Notion")
      icon_result="󰈙"
      ;;
    "Obsidian")
      icon_result="󰠮"
      ;;
    "Craft")
      icon_result="󰈙"
      ;;
    "Bear")
      icon_result="󰈙"
      ;;
    "Ulysses")
      icon_result="󰈙"
      ;;
    "Day One")
      icon_result="󰃭"
      ;;
    "Things" | "Things 3")
      icon_result="󰄲"
      ;;
    "OmniFocus")
      icon_result="󰄲"
      ;;
    "Todoist")
      icon_result="󰄲"
      ;;
    "Fantastical" | "Cron" | "Amie" | "Morgen")
      icon_result="󰃭"
      ;;
    "Cardhop")
      icon_result="󰛋"
      ;;
    "Drafts")
      icon_result="󰈙"
      ;;
    "GoodNotes")
      icon_result="󰈙"
      ;;
    "Notability")
      icon_result="󰈙"
      ;;
    "MindNode")
      icon_result="󰈙"
      ;;
    "OmniGraffle")
      icon_result="󰽉"
      ;;
    "OmniPlan")
      icon_result="󰠵"
      ;;
    "OmniOutliner")
      icon_result="󰈙"
      ;;
    "Microsoft To Do")
      icon_result="󰄲"
      ;;
    "TickTick")
      icon_result="󰄲"
      ;;
    "Logseq")
      icon_result="󰈙"
      ;;
    "Joplin")
      icon_result="󰈙"
      ;;
    "Evernote" | "Evernote Legacy")
      icon_result="󰈙"
      ;;
    "DEVONthink 3")
      icon_result="󰈙"
      ;;
    "Yuque" | "语雀")
      icon_result="󰈙"
      ;;
    "ClickUp")
      icon_result="󰄲"
      ;;
    "Linear")
      icon_result="󰈙"
      ;;
    "Trello")
      icon_result="󰠵"
      ;;
    "Reeder" | "Reeder 5")
      icon_result="󰑫"
      ;;
    "PomoDone App")
      icon_result="󰥔"
      ;;
    
    # Microsoft Office
    "Microsoft Word")
      icon_result="󰈙"
      ;;
    "Microsoft Excel")
      icon_result="󰈛"
      ;;
    "Microsoft PowerPoint")
      icon_result="󰈙"
      ;;
    "Microsoft Outlook")
      icon_result="󰴢"
      ;;
    "Microsoft OneNote")
      icon_result="󰈙"
      ;;
    
    # Apple iWork
    "Pages" | "Pages 文稿")
      icon_result="󰈙"
      ;;
    "Numbers" | "Numbers 表格")
      icon_result="󰈛"
      ;;
    "Keynote" | "Keynote 讲演")
      icon_result="󰈙"
      ;;
    
    # Creative Tools
    "Figma")
      icon_result="󰉼"
      ;;
    "Sketch")
      icon_result="󰉼"
      ;;
    "Adobe Photoshop" | "Photoshop")
      icon_result="󰈙"
      ;;
    "Adobe Illustrator" | "Illustrator")
      icon_result="󰈙"
      ;;
    "Adobe InDesign" | "InDesign")
      icon_result="󰈙"
      ;;
    "Adobe XD")
      icon_result="󰈙"
      ;;
    "Adobe Premiere Pro" | "Premiere Pro")
      icon_result="󰈙"
      ;;
    "Adobe After Effects" | "After Effects")
      icon_result="󰈙"
      ;;
    "Adobe Lightroom" | "Lightroom")
      icon_result="󰈙"
      ;;
    "Adobe Acrobat" | "Acrobat Reader")
      icon_result="󰈙"
      ;;
    "Affinity Photo")
      icon_result="󰈙"
      ;;
    "Affinity Designer")
      icon_result="󰈙"
      ;;
    "Affinity Publisher")
      icon_result="󰈙"
      ;;
    "Pixelmator Pro")
      icon_result="󰈙"
      ;;
    "Canva")
      icon_result="󰈙"
      ;;
    "Procreate")
      icon_result="󰈙"
      ;;
    "Final Cut Pro")
      icon_result="󰈙"
      ;;
    "Motion")
      icon_result="󰈙"
      ;;
    "Compressor")
      icon_result="󰈙"
      ;;
    "Logic Pro" | "Logic Pro X")
      icon_result="󰎆"
      ;;
    "GarageBand")
      icon_result="󰎆"
      ;;
    "MainStage")
      icon_result="󰎆"
      ;;
    "DaVinci Resolve")
      icon_result="󰈙"
      ;;
    "OBS" | "OBS Studio")
      icon_result="󰕧"
      ;;
    "ScreenFlow")
      icon_result="󰕧"
      ;;
    "Camtasia")
      icon_result="󰕧"
      ;;
    "Blender")
      icon_result="󰂫"
      ;;
    "Zeplin")
      icon_result="󰈙"
      ;;
    "Color Picker" | "数码测色计")
      icon_result="󰏘"
      ;;
    "Live" | "Ableton Live")
      icon_result="󰎆"
      ;;
    "Audacity")
      icon_result="󰎙"
      ;;
    
    # Media Players
    "Spotify")
      icon_result="󰓇"
      ;;
    "Apple Music" | "Music" | "音乐")
      icon_result="󰎆"
      ;;
    "iTunes")
      icon_result="󰎆"
      ;;
    "Tidal" | "TIDAL")
      icon_result="󰎆"
      ;;
    "YouTube Music")
      icon_result="󰎦"
      ;;
    "Vox")
      icon_result="󰎆"
      ;;
    "VLC")
      icon_result="󰕧"
      ;;
    "IINA")
      icon_result="󰕧"
      ;;
    "Plex")
      icon_result="󰈙"
      ;;
    "Infuse")
      icon_result="󰈙"
      ;;
    "TV" | "Apple TV")
      icon_result="󰺵"
      ;;
    "Podcasts" | "播客")
      icon_result="󰦔"
      ;;
    "Overcast")
      icon_result="󰦔"
      ;;
    "Pocket Casts")
      icon_result="󰦔"
      ;;
    "Castro")
      icon_result="󰦔"
      ;;
    "网易云音乐" | "NetEase Music")
      icon_result="󰎆"
      ;;
    "Jellyfin Media Player")
      icon_result="󰕧"
      ;;
    
    # Utilities
    "1Password" | "1Password 7")
      icon_result="󰌾"
      ;;
    "Bitwarden")
      icon_result="󰌾"
      ;;
    "LastPass")
      icon_result="󰌾"
      ;;
    "Dashlane")
      icon_result="󰌾"
      ;;
    "KeePassXC")
      icon_result="󰌾"
      ;;
    "Alfred")
      icon_result="󰈙"
      ;;
    "Raycast")
      icon_result="󰈙"
      ;;
    "LaunchBar")
      icon_result="󰈙"
      ;;
    "Bartender")
      icon_result="󰂵"
      ;;
    "Rectangle")
      icon_result="󰁌"
      ;;
    "Magnet")
      icon_result="󰁌"
      ;;
    "BetterTouchTool")
      icon_result="󰈙"
      ;;
    "Karabiner-Elements")
      icon_result="󰌌"
      ;;
    "AeroSpace")
      icon_result="󰊠"
      ;;
    "CleanMyMac X")
      icon_result="󰃢"
      ;;
    "AppCleaner")
      icon_result="󰃢"
      ;;
    "The Unarchiver")
      icon_result="󰀼"
      ;;
    "Keka")
      icon_result="󰀼"
      ;;
    "Transmission")
      icon_result="󰇚"
      ;;
    "qBittorrent")
      icon_result="󰇚"
      ;;
    "Amphetamine")
      icon_result="󰒲"
      ;;
    "Lungo")
      icon_result="󰒲"
      ;;
    "F.lux")
      icon_result="󰖨"
      ;;
    "Night Owl")
      icon_result="󰖔"
      ;;
    "Clipboard Manager")
      icon_result="󰅇"
      ;;
    "Paste")
      icon_result="󰅇"
      ;;
    "PopClip")
      icon_result="󰈙"
      ;;
    "TextExpander")
      icon_result="󰈙"
      ;;
    "Hazel")
      icon_result="󰈙"
      ;;
    "Keyboard Maestro")
      icon_result="󰈙"
      ;;
    "Little Snitch")
      icon_result="󰒍"
      ;;
    "Micro Snitch")
      icon_result="󰍀"
      ;;
    "iStat Menus")
      icon_result="󰍛"
      ;;
    "MenuMeters")
      icon_result="󰍛"
      ;;
    "Timing")
      icon_result="󰥔"
      ;;
    "RescueTime")
      icon_result="󰥔"
      ;;
    "Toggl Track")
      icon_result="󰥔"
      ;;
    "Clockify")
      icon_result="󰥔"
      ;;
    "Spotlight")
      icon_result="󰍉"
      ;;
    "Iris")
      icon_result="󰖨"
      ;;
    "Grammarly Editor")
      icon_result="󰈙"
      ;;
    "Setapp")
      icon_result="󰣆"
      ;;
    "Pi-hole Remote")
      icon_result="󰒍"
      ;;
    
    # File Management
    "Path Finder")
      icon_result="󰉋"
      ;;
    "ForkLift")
      icon_result="󰉋"
      ;;
    "Commander One")
      icon_result="󰉋"
      ;;
    "Transmit")
      icon_result="󰒍"
      ;;
    "Cyberduck")
      icon_result="󰒍"
      ;;
    "FileZilla")
      icon_result="󰒍"
      ;;
    "CloudMounter")
      icon_result="󰅧"
      ;;
    "ExpanDrive")
      icon_result="󰅧"
      ;;
    "Dropbox")
      icon_result="󰇣"
      ;;
    "Google Drive")
      icon_result="󰊷"
      ;;
    "OneDrive")
      icon_result="󰂩"
      ;;
    "Box")
      icon_result="󰈙"
      ;;
    "Sync")
      icon_result="󰓦"
      ;;
    "MEGA")
      icon_result="󰈙"
      ;;
    "pCloud Drive")
      icon_result="󰅧"
      ;;
    "Folx")
      icon_result="󰇚"
      ;;
    
    # Games & Entertainment
    "Steam")
      icon_result="󰓓"
      ;;
    "Epic Games Launcher")
      icon_result="󰊴"
      ;;
    "Battle.net")
      icon_result="󰺵"
      ;;
    "GOG Galaxy")
      icon_result="󰊴"
      ;;
    "GeForce NOW")
      icon_result="󰺵"
      ;;
    "Xbox")
      icon_result="󰊴"
      ;;
    "PlayStation Remote Play")
      icon_result="󰊴"
      ;;
    "OpenEmu")
      icon_result="󰊴"
      ;;
    "Minecraft")
      icon_result="󰍳"
      ;;
    "League of Legends")
      icon_result="󰊴"
      ;;
    "Godot")
      icon_result="󰐣"
      ;;
    
    # Finance
    "Mint")
      icon_result="󰈙"
      ;;
    "YNAB")
      icon_result="󰈙"
      ;;
    "Quicken")
      icon_result="󰈙"
      ;;
    "MoneyMoney")
      icon_result="󰈙"
      ;;
    "Stocks")
      icon_result="󰈙"
      ;;
    "Crypto Pro")
      icon_result="󰠓"
      ;;
    "Coinbase")
      icon_result="󰠓"
      ;;
    "Binance")
      icon_result="󰠓"
      ;;
    "GrandTotal" | "Receipts")
      icon_result="󰈙"
      ;;
    
    # Email Clients
    "Canary Mail")
      icon_result="󰊫"
      ;;
    "HEY")
      icon_result="󰊫"
      ;;
    "Mailspring")
      icon_result="󰊫"
      ;;
    "MailMate")
      icon_result="󰊫"
      ;;
    "Spark" | "Spark Desktop")
      icon_result="󰊫"
      ;;
    "Tweetbot" | "Twitter")
      icon_result="󰔁"
      ;;
    
    # Development Related
    "MAMP" | "MAMP PRO")
      icon_result="󱄙"
      ;;
    "Matlab")
      icon_result="󰈙"
      ;;
    "Cypress")
      icon_result="󰈙"
      ;;
    "Replit")
      icon_result="󰈙"
      ;;
    "Pine")
      icon_result="󰈙"
      ;;
    
    # Reading & Books
    "Calibre")
      icon_result="󰂺"
      ;;
    "Skim")
      icon_result="󰈙"
      ;;
    "zathura")
      icon_result="󰈙"
      ;;
    "Zotero")
      icon_result="󰈙"
      ;;
    "Typora")
      icon_result="󰈙"
      ;;
    
    # Default/Unknown Apps
    *)
      # Try to guess based on common patterns
      case "${app,,}" in
        *terminal*|*term*|*console*)     icon_result="󰆍" ;;
        *code*|*editor*|*ide*)           icon_result="󰨞" ;;
        *git*|*version*)                 icon_result="󰊢" ;;
        *docker*|*container*)            icon_result="󰡨" ;;
        *database*|*sql*|*db*)           icon_result="󰆼" ;;
        *browser*|*web*)                 icon_result="󰈹" ;;
        *mail*|*email*)                  icon_result="󰊫" ;;
        *message*|*chat*|*talk*)         icon_result="󰍡" ;;
        *music*|*audio*|*sound*)         icon_result="󰎆" ;;
        *video*|*movie*|*player*)        icon_result="󰕧" ;;
        *photo*|*image*|*picture*)       icon_result="󰋣" ;;
        *calendar*|*schedule*)           icon_result="󰃭" ;;
        *note*|*memo*)                   icon_result="󰎞" ;;
        *task*|*todo*|*reminder*)        icon_result="󰄲" ;;
        *download*)                      icon_result="󰇚" ;;
        *vpn*|*proxy*|*tunnel*)          icon_result="󰒍" ;;
        *backup*|*sync*)                 icon_result="󰓦" ;;
        *clean*|*optimize*)              icon_result="󰃢" ;;
        *archive*|*zip*|*compress*)      icon_result="󰀼" ;;
        *settings*|*preferences*)        icon_result="󰒓" ;;
        *monitor*|*stat*)                icon_result="󰍛" ;;
        *security*|*antivirus*)          icon_result="󰒃" ;;
        *password*|*vault*)              icon_result="󰌾" ;;
        *)                               icon_result="󰣆" ;; # Generic app icon
      esac
      ;;
  esac
}

# Export the function so it can be used by other scripts
export -f __icon_map
