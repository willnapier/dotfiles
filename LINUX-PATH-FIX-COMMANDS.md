# Linux PATH Fix - Permanent Solution Commands

## Problem
Nushell can't start properly because `/home/will/.cargo/bin` isn't in system PATH, creating circular dependency where env.nu can't load to set PATH.

## Solution - Run these commands on Linux machine (requires sudo):

### Step 1: System-Wide Environment Configuration
```bash
sudo tee /etc/environment.d/50-user-paths.conf << 'EOF'
PATH=/home/will/.cargo/bin:/home/will/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/bin
EOF
```

### Step 2: PAM Environment Setup
```bash
sudo tee -a /etc/pam.d/common-session << 'EOF'
session required pam_env.so readenv=1 user_readenv=1
EOF
```

### Step 3: SSH Daemon Configuration
```bash
sudo tee -a /etc/ssh/sshd_config << 'EOF'
# Accept environment variables for user paths
AcceptEnv PATH
EOF

sudo systemctl restart sshd
```

### Step 4: Create System-Wide Profile Script
```bash
sudo tee /etc/profile.d/nushell-path.sh << 'EOF'
#!/bin/bash
# Ensure Nushell and user scripts are in PATH for all users
if [ "$USER" = "will" ]; then
    export PATH="/home/will/.cargo/bin:/home/will/.local/bin:$PATH"
fi
EOF

sudo chmod +x /etc/profile.d/nushell-path.sh
```

### Step 5: Test
After running commands, restart terminal or reboot, then test:
```bash
echo $PATH
which zj
zj desktop
```

## Expected Result
- `zj` command found automatically
- Full debug output from zj script
- Zellij sessions create properly with layouts

## Status
- [x] Ghostty scrolling fixed (added `alternate-screen-scroll = true`)
- [ ] Linux PATH commands need to be run on nimbini