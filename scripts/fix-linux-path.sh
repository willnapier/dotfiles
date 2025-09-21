#!/bin/bash
# Linux PATH Fix Script - Run with sudo

echo "Creating system-wide PATH configuration..."

# Step 1: System Environment Configuration
mkdir -p /etc/environment.d
cat > /etc/environment.d/50-user-paths.conf << 'EOF'
PATH=/home/will/.cargo/bin:/home/will/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/bin
EOF

# Step 2: PAM Environment Setup
echo 'session required pam_env.so readenv=1 user_readenv=1' >> /etc/pam.d/common-session

# Step 3: SSH Configuration
cat >> /etc/ssh/sshd_config << 'EOF'
# Accept environment variables for user paths
AcceptEnv PATH
EOF

systemctl restart sshd

# Step 4: Profile Script Fallback
cat > /etc/profile.d/nushell-path.sh << 'EOF'
#!/bin/bash
# Ensure Nushell and user scripts are in PATH for all users
if [ "$USER" = "will" ]; then
    export PATH="/home/will/.cargo/bin:/home/will/.local/bin:$PATH"
fi
EOF

chmod +x /etc/profile.d/nushell-path.sh

echo "Linux PATH fix completed successfully!"
echo "Please exit and reconnect to test."