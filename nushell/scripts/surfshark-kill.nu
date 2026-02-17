#!/usr/bin/env nu
# Kill Surfshark app process and WireGuard tunnel (quick TouchID fix)

^sudo pkill -9 Surfshark
^sudo pkill -9 -f PacketTunnel-WireGuard
