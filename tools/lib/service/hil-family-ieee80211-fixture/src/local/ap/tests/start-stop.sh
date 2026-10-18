#!/bin/bash
# Execute the real AP setup/cleanup with only OS/network operations substituted.
source "$1" capabilities >/dev/null
STATE_DIR=$2/state
CTRL_DIR=$2/control
AP_CONFIG=$STATE_DIR/hostapd.conf
WPA_CONFIG=$STATE_DIR/wpa.conf
WPA_CTRL=$STATE_DIR/wpa-control
WPA_PID=$STATE_DIR/wpa.pid
HOSTAPD_PID=$STATE_DIR/hostapd.pid
DNSMASQ_PID=$STATE_DIR/dnsmasq.pid
HOSTAPD=$2/hostapd
printf '#!/bin/sh\ntest -f "$2"\n' >"$HOSTAPD"
chmod 0700 "$HOSTAPD"
take_network_manager_ownership() { :; }
remember_wifi_identity() { :; }
acquire_wifi_rfkill() { :; }
restore_wifi_rfkill() { :; }
restore_managed_interface() { :; }
/usr/bin/ip() { :; }
/usr/bin/iw() { :; }
/usr/bin/wpa_cli() { :; }
/bin/chown() { :; }
/usr/bin/dnsmasq() { printf '%s\n' "$@" >"$STATE_DIR/dhcp-args"; }
start_ap
wait "$(cat "$HOSTAPD_PID")"
test "$(stat -c %a "$AP_CONFIG")" = 600
test "$(stat -c %a "$STATE_DIR")" = 700
test "$(stat -c %a "$CTRL_DIR/hostapd.log")" = 640
cat "$STATE_DIR/dhcp-args"
stop_radio_services
test ! -e "$AP_CONFIG"
test ! -e "$CTRL_DIR/hostapd.log"
