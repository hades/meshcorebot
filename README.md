MeshcoreBot
===========

This is a simple Meshcore bot that listens to "ping" messages and responds with
the number of hops and the current location.

This illustrates how to use the `meshcore-rs` crate, and can potentially be
extended to provide more functions.

Usage
-----

First, pair your device using your system's Bluetooth functionality. This is
platform-specific, and you need to make sure you use the correct PIN. Some
systems will try to pair with the default PIN, which is usually not what you
want.

For example, in Linux you may try:

```
$ bluetoothctl
# scan le
[NEW] Device de:ad:be:ef:12:34 MeshCore-MyDevice
# scan off
# pair MeshCore-MyDevice
```


Having paired the device, you may run the bot. Configuration options are
provided via environment variables. For example:

```
$ MESHCOREBOT_LOG=info \
  MESHCOREBOT_DEVICE=MeshCore-Bubatz \
  MESHCOREBOT_CHANNEL=#test \
  MESHCOREBOT_LOCATION="Berlin Mitte JO62QM84" \
  cargo run
```
