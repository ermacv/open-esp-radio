// OpenWrt IPC boundary. Profile selection, validation and ownership live in Rust.
// `request` is passed on SSH stdin, never in argv or a persistent remote file.
import { cursor } from 'uci';
import { connect, error as bus_error } from 'ubus';
import * as loop from 'uloop';
import { popen, readfile } from 'fs';

function checked(value, message) {
    if (value == null || value == false)
        die(message + '\n');
    return value;
}
let config = cursor();
let bus = checked(connect(null, 5), 'cannot connect to ubus');
let radio = request.radio;
let section = request.ap_section;
checked(config.get('wireless', radio) == 'wifi-device', 'unknown UCI radio');
checked(config.get('wireless', section) == 'wifi-iface', 'unknown UCI AP section');
checked(config.get('wireless', section, 'device') == radio, 'AP section belongs to another radio');
let object = 'hostapd.' + request.interface;
function call(object, method, arguments) {
    let value = bus.call(object, method, arguments);
    let error = bus_error();
    if (error)
        die('ubus ' + object + '.' + method + ': ' + error + '\n');
    return value;
}

function snapshot() {
    let state = checked(bus.call('network.wireless', 'status', { device: radio }), 'netifd status failed')[radio];
    let active = bus.call(object, 'get_status', {});
    let options = {};
    let pending = {};
    let changes = config.changes('wireless').wireless || [];
    for (let name, values in request.options) {
        options[name] = {};
        pending[name] = {};
        for (let key in values) {
            options[name][key] = config.get('wireless', name, key);
            pending[name][key] = length(filter(changes, (c) => c[1] == name && c[2] == key)) > 0;
        }
    }
    return { up: state.up, ap_enabled: active?.status == 'ENABLED', options, pending };
}

if (request.operation == 'snapshot') {
    print(sprintf('%J\n', snapshot()));
    exit(0);
}

if (request.operation == 'verify') {
    let active = checked(bus.call(object, 'get_status', {}), 'existing AP unavailable');
    let wanted = request.options[section];
    checked(active.status == 'ENABLED' && active.ssid == wanted.ssid, 'existing AP identity differs');
    let defaults = { wmm: '1', ieee80211w: '0' };
    for (let key in ['mode', 'ssid', 'key', 'encryption', 'wmm', 'ieee80211w']) {
        let current = config.get('wireless', section, key) ?? defaults[key];
        checked(current == wanted[key], 'existing AP settings differ: ' + key);
    }
    print(sprintf('%J\n', snapshot()));
    exit(0);
}

if (request.operation == 'observe') {
    let status = checked(bus.call(object, 'get_status', {}), 'hostapd status unavailable');
    let phy = checked(bus.call('iwinfo', 'phyname', { section: radio }), 'cannot resolve wiphy').phyname;
    // Names are validated by the Rust configuration loader before use in commands.
    let info = popen('iw dev ' + request.interface + ' info', 'r');
    let geometry = join('\n', filter(split(info.read('all'), '\n'), (line) => match(line, /^\s*channel /)));
    checked(info.close() == 0, 'cannot read active channel geometry');
    let file = readfile('/var/run/hostapd-' + phy + '.conf');
    checked(file != null, 'cannot read generated hostapd mode');
    print(sprintf('%J\n', {
        enabled: status.status == 'ENABLED', channel: status.channel,
        geometry, htmode: config.get('wireless', radio, 'htmode'),
        ht: match(file, /(^|\n)ieee80211n=1(\n|$)/) != null,
        he: match(file, /(^|\n)ieee80211ax=1(\n|$)/) != null,
        beacon_interval_tu: int(match(file, /(^|\n)beacon_int=([0-9]+)/)?.[2]) ?? null,
        dtim_period: int(match(file, /(^|\n)dtim_period=([0-9]+)/)?.[2]) ?? null
    }));
    exit(0);
}

// Subscribe before changing state; the deadline bounds failure, never readiness.
loop.init();
let ready = false;
let last_state = null;
let phase = 'state';
function inspect() {
    let state = bus.call('network.wireless', 'status', { device: radio });
    if (!state || !state[radio])
        return;
    let status = bus.call(object, 'get_status', {});
    last_state = { up: state[radio].up, pending: state[radio].pending, hostapd: status?.status };
    // Restoration can intentionally remove our AP while keeping other BSSes
    // on this radio up. Snapshot comparison verifies its original AP state.
    let restoring = request.operation == 'restore';
    let ap_matches = restoring
        ? (status?.status == 'ENABLED') == request.ap_enabled
        : status?.status == 'ENABLED';
    ready = phase == 'configuration' ? !state[radio].pending : !state[radio].pending && (request.up
        ? state[radio].up && ap_matches
        : !status && !state[radio].up);
    if (ready && request.up && !restoring && phase == 'state') {
        // An enabled netdev from the old configuration is not the new AP.
        ready = status.ssid == config.get('wireless', section, 'ssid') &&
            status.channel == int(config.get('wireless', radio, 'channel')) &&
            state[radio].config.htmode == config.get('wireless', radio, 'htmode');
    }
    if (ready)
        loop.end();
}
let listener = checked(bus.listener('ubus.object.*', () => inspect()), 'cannot subscribe to ubus objects');
let subscriber = checked(bus.subscriber(() => inspect()), 'cannot create hostapd subscriber');
checked(subscriber.subscribe('hostapd'), 'cannot subscribe to hostapd lifecycle');
// BSS notifications precede netifd's final setup/teardown bookkeeping.
// netifd.wireless.done is delivered through procd's service event.trigger,
// not through hostapd or a network.wireless broadcast.
let service = checked(bus.subscriber(() => inspect()), 'cannot create netifd completion subscriber');
checked(service.subscribe('service'), 'cannot subscribe to netifd completion events');
let deadline = loop.timer(20000, () => loop.end());
function wait_state() {
    ready = false;
    inspect();
    if (!ready)
        loop.run();
    checked(ready, phase + ' readiness deadline: ' + sprintf('%J', last_state));
}
if (request.operation == 'apply' || request.operation == 'restore') {
    for (let name, values in request.options) {
        for (let key, value in values) {
            // The scenario's changes are temporary; restoring reverts them
            // to the committed configuration the snapshot found.
            if (request.operation == 'restore')
                config.revert('wireless', name, key);
            else if (value == null)
                config.delete('wireless', name, key);
            else
                checked(config.set('wireless', name, key, value), 'cannot set UCI option');
        }
    }
    checked(config.save('wireless'), 'cannot save temporary UCI options');
    // netifd owns configuration reload; unchanged radios retain their configuration.
    phase = 'configuration';
    call('network', 'reload', {});
    wait_state();
}
// A down request racing an unfinished reload can cancel only that setup, then
// be superseded by its queued restart. Set the final state after reload settles.
phase = 'state';
call('network.wireless', request.up ? 'up' : 'down', { device: radio });
wait_state();
print(sprintf('%J\n', snapshot()));
