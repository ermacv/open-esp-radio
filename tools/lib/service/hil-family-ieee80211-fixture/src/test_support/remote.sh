# Command substitutes for the fixture owner's real remote programs.
nft() {
    case "$*" in
        'list tables')
            if test "$OER_TEST_CASE" = cleanup-inspection-error; then return 1; fi
            echo 'table inet fw4'
            if test -f "$OER_TEST_STATE/nat"; then echo 'table inet open_radio_hil'; fi;;
        'delete table inet open_radio_hil')
            case "$OER_TEST_CASE" in cleanup-nft-error) return 1;; cleanup-nft-noop) return 0;; esac
            command rm -f "$OER_TEST_STATE/nat";;
        '-a list table inet fw4')
            echo 'chain forward {'
            if test -f "$OER_TEST_STATE/chain"; then echo 'chain open_radio_hil_forward {'; fi;;
        '-a list chain inet fw4 forward') echo 'jump open_radio_hil_forward # handle 42';;
        'delete rule inet fw4 forward handle 42'|'flush chain inet fw4 open_radio_hil_forward') :;;
        'delete chain inet fw4 open_radio_hil_forward') command rm -f "$OER_TEST_STATE/chain";;
        *) echo "unexpected nft: $*" >&2; return 1;;
    esac
    return 0
}
test() {
    case "$*" in
        '-f /var/run/open-radio-client.pid'|'-d /proc/99') command test -f "$OER_TEST_STATE/process"; return $?;;
    esac
    command test "$@"
}
cat() {
    case "$*" in
        /var/run/open-radio-client.pid) echo 99;;
        /proc/99/cmdline)
            if test "$OER_TEST_CASE" = cleanup-process-mismatch; then
                printf '/usr/sbin/wpa_supplicant\000-i\000another-client\000-c\000/unrelated.conf\000'
            else
                printf '/usr/sbin/wpa_supplicant\000-i\000or-ap-client\000-c\000/var/run/open-radio-client.conf\000'
            fi;;
        *) command cat "$@";;
    esac
}
uci() {
    case "$*" in
        '-q show wireless') echo 'wireless.ap=wifi-iface';;
        '-q get wireless.ap.device') echo radio0;;
        '-q get wireless.ap.ssid') echo test-network;;
        '-q get wireless.ap.key') echo test-password;;
        *) return 1;;
    esac
}
kill() {
    case "$OER_TEST_CASE" in monitor-*) command kill "$@"; return;; esac
    touch "$OER_TEST_STATE/kill-attempted"
    case "$OER_TEST_CASE" in cleanup-process-error) return 1;; cleanup-process-noop) return 0;; esac
    command rm -f "$OER_TEST_STATE/process"
}
rm() {
    case "$*" in *open-radio-client*) return 0;; esac
    command rm "$@"
}
wifi() {
    if test "$OER_TEST_CASE" = ap-command-error; then return 1; fi
    printf %s "$1" > "$OER_TEST_STATE/wireless"
}
iw() {
    case "$*" in
        dev)
            echo 'Interface external-observer'
            if test -f "$OER_TEST_STATE/client"; then echo 'Interface or-ap-client'; fi
            return 0;;
        'dev phy0-ap0 info')
            if test "$OER_TEST_CASE" = ap-command-error; then
                return 1
            else
                printf 'wiphy 0\nchannel 6 (2437 MHz), width: 40 MHz\n'
            fi;;
        'dev open-radio-mon info') test -f "$OER_TEST_STATE/monitor";;
        'dev open-radio-mon del') command rm -f "$OER_TEST_STATE/monitor";;
        'phy phy0 interface add open-radio-mon type monitor') echo owned > "$OER_TEST_STATE/monitor";;
        'dev or-ap-client del')
            case "$OER_TEST_CASE" in cleanup-iw-error) return 1;; cleanup-iw-noop) return 0;; esac
            command rm -f "$OER_TEST_STATE/client";;
        *) echo "unexpected iw: $*" >&2; return 1;;
    esac
}
ip() {
    case "$*" in
        'neigh show '*) echo '192.0.2.2 lladdr 02:00:00:00:00:01 REACHABLE';;
        'link set open-radio-mon up')
            case "$OER_TEST_CASE" in
                monitor-error) return 1;;
                monitor-cancel) touch "$OER_TEST_STATE/ready"; command sleep 20;;
            esac;;
        *) return 1;;
    esac
}
mkdir() {
    printf '%s\n' "$1" > "$OER_TEST_STATE/remote-directory"
    command mkdir "$@"
}
timeout() {
    shift 3
    test "$1" = sh && test "$2" = -c || return 1
    eval "$3"
}
tcpdump() {
    printf '%s\n' "$@" > "$OER_TEST_STATE/capture-arguments"
    trap 'exit 0' TERM
    printf 'tcpdump: listening on open-radio-mon, link-type IEEE802_11_RADIO\n' >&2
    touch "$OER_TEST_STATE/ready"
    command sleep 20
}
