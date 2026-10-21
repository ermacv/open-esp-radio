#!/usr/bin/env python3
"""W3 laptop station. A volatile NetworkManager connection; no profiles/logs.

The parent holds the physical radio and exclusive air leases. Configuration
arrives on stdin, never argv. Only IPv4 lease and aggregate ping counts leave
this process. D-Bus client lifetime binds the temporary connection to us.
"""

import ipaddress
import json
import re
import signal
import subprocess
import sys
import time
import uuid

import dbus

stage = "configuration"


def interrupted(number, _frame):
    raise SystemExit(128 + number)


signal.signal(signal.SIGTERM, interrupted)
signal.signal(signal.SIGINT, interrupted)

def emit(value):
    print(json.dumps(value), flush=True)


def main():
    global stage
    config = json.loads(sys.stdin.readline(1025))
    if set(config) != {"ssid", "channel"}:
        raise ValueError("invalid configuration")
    ssid = config["ssid"].encode("ascii")
    channel = config["channel"]
    if not 1 <= len(ssid) <= 32 or not isinstance(channel, int) or not 1 <= channel <= 13:
        raise ValueError("invalid configuration")

    bus = dbus.SystemBus(private=True)
    stage = "device"
    service = "org.freedesktop.NetworkManager"
    properties = "org.freedesktop.DBus.Properties"
    manager_object = bus.get_object(service, "/org/freedesktop/NetworkManager")
    manager = dbus.Interface(manager_object, service)
    manager_properties = dbus.Interface(manager_object, properties)
    enabled = bool(manager_properties.Get(service, "WirelessEnabled"))
    device_path = manager.GetDeviceByIpIface("wlan0")
    device = dbus.Interface(bus.get_object(service, device_path), properties)
    device_type = service + ".Device"
    if str(device.Get(device_type, "ActiveConnection")) != "/":
        raise RuntimeError("laptop radio already has an active connection")
    managed = bool(device.Get(device_type, "Managed"))
    active = None
    try:
        stage = "activation"
        manager_properties.Set(service, "WirelessEnabled", dbus.Boolean(True))
        device.Set(device_type, "Managed", dbus.Boolean(True))
        deadline = time.monotonic() + 10
        while int(device.Get(device_type, "State")) < 30:
            if time.monotonic() > deadline:
                print("W3 laptop radio state=" + str(int(device.Get(device_type, "State"))), file=sys.stderr)
                raise RuntimeError("station device unavailable")
            time.sleep(0.1)
        wireless = dbus.Interface(bus.get_object(service, device_path), service + ".Device.Wireless")
        stage = "scan"
        wireless.RequestScan(dbus.Dictionary({}, signature="sv"))
        access_point = None
        deadline = time.monotonic() + 20
        while access_point is None:
            for candidate in wireless.GetAllAccessPoints():
                ap = dbus.Interface(bus.get_object(service, candidate), properties)
                if bytes(ap.Get(service + ".AccessPoint", "Ssid")) == ssid:
                    access_point = candidate
                    break
            if time.monotonic() > deadline:
                raise RuntimeError("station scan timed out")
            time.sleep(0.1)
        stage = "activation"
        settings = {
            "connection": {"id": "oer-w3-ax211-" + uuid.uuid4().hex, "uuid": str(uuid.uuid4()), "type": "802-11-wireless", "autoconnect": dbus.Boolean(False)},
            "802-11-wireless": {"ssid": dbus.ByteArray(ssid), "mode": "infrastructure", "band": "bg", "channel": dbus.UInt32(channel), "powersave": dbus.UInt32(2), "assigned-mac-address": "permanent"},
            "ipv4": {"method": "auto", "never-default": dbus.Boolean(True), "ignore-auto-dns": dbus.Boolean(True)},
            "ipv6": {"method": "disabled"},
        }
        _, active, _ = manager.AddAndActivateConnection2(
            settings, device_path, access_point,
            {"persist": "volatile", "bind-activation": "dbus-client"},
            signature="a{sa{sv}}ooa{sv}",
        )
        deadline = time.monotonic() + 45
        stage = "DHCP"
        while int(device.Get(device_type, "State")) != 100:
            if time.monotonic() > deadline:
                raise RuntimeError("station DHCP timed out")
            time.sleep(0.1)
        ip_path = device.Get(device_type, "Ip4Config")
        ip = dbus.Interface(bus.get_object(service, ip_path), properties)
        values = ip.GetAll(service + ".IP4Config")
        addresses = values["AddressData"]
        if len(addresses) != 1:
            raise RuntimeError("station requires one DHCP address")
        address = str(ipaddress.IPv4Address(str(addresses[0]["address"])))
        dhcp = dbus.Interface(bus.get_object(service, device.Get(device_type, "Dhcp4Config")), properties)
        routers = str(dhcp.Get(service + ".DHCP4Config", "Options")["routers"]).split()
        if len(routers) != 1:
            raise RuntimeError("station requires one DHCP router")
        router = str(ipaddress.IPv4Address(routers[0]))
        emit({"address": address, "router": router, "prefix": int(addresses[0]["prefix"])})
        if sys.stdin.readline(32).strip() != "ping":
            raise RuntimeError("expected ping command")
        stage = "ping"
        ping = subprocess.run(["ping", "-n", "-c", "100", "-i", "1", "-W", "1", "-I", "wlan0", router], capture_output=True, text=True, timeout=115)
        matches = re.findall(r"^(\d+) packets transmitted, (\d+) received,", ping.stdout, re.MULTILINE)
        if len(matches) != 1 or ping.returncode not in (0, 1):
            raise RuntimeError("invalid station ping result")
        sent, received = map(int, matches[0])
        emit({"sent": sent, "received": received, "interval_ms": 1000})
        if sys.stdin.readline(32).strip() != "disconnect":
            raise RuntimeError("expected disconnect command")
    except Exception as error:
        name = error.get_dbus_name() if isinstance(error, dbus.DBusException) else type(error).__name__
        hints = [word for word in ("unmanaged", "not available", "not compatible", "permission", "ssid", "unsupported", "wireless", "access point", "unknown", "invalid", "not allowed") if word in str(error).lower()]
        print(f"W3 laptop {stage}: {name}; flags={','.join(hints)}", file=sys.stderr)
        raise
    finally:
        stage = "cleanup"
        try:
            if active is not None and device.Get(device_type, "ActiveConnection") == active:
                manager.DeactivateConnection(active)
                deadline = time.monotonic() + 15
                while str(device.Get(device_type, "ActiveConnection")) != "/":
                    if time.monotonic() > deadline:
                        raise RuntimeError("station cleanup timed out")
                    time.sleep(0.1)
        finally:
            try:
                device.Set(device_type, "Managed", dbus.Boolean(managed))
                manager_properties.Set(service, "WirelessEnabled", dbus.Boolean(enabled))
            finally:
                bus.close()
    emit({"disconnected": True})


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # D-Bus errors may include SSIDs, interfaces or full MAC addresses.
        print(f"W3 laptop {stage} failed", file=sys.stderr)
        sys.exit(1)
