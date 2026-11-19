# Device inventory fixtures

Fixtures for `istesso devices list` (M7 step 1). Each one has the real shape of the OS output
on the Mac (a MacBook Air, macOS 26, Japanese locale) or on the host (Ubuntu), taken on
2026-11-19, with every identifying value replaced. The raw output was never written to a file:
`sanitize.py` runs the macOS commands itself and keeps their output in memory; on Linux the
output of `collect-sysfs.sh` goes from ssh straight into `sanitize.py` through a pipe.
These two tools are not part of the repository: they ran once, outside it, on the owner's machines.
The fixtures were then checked with `scripts/check-private.sh` and by a separate reviewer who
compared them, in memory, with the live output.

Placeholders are consistent across all macOS fixtures of one run (one process), so the joins
the code makes still work: the same vendor id, product id, location id or product name maps to
the same placeholder in system_profiler, the USB device records, the interface records and HID.

## Files

| File | Source command | Shape |
|---|---|---|
| `mac-system_profiler.json` | `system_profiler -json SPUSBHostDataType SPCameraDataType SPAudioDataType SPBluetoothDataType` | JSON object, one array per data type; USB as nested `_items` trees; Bluetooth `device_connected` / `device_not_connected` as arrays of one-key objects keyed by the device name |
| `mac-ioreg-usb.plist` | `ioreg -a -l -r -c IOUSBHostDevice -d 1` | XML plist, array of one dict per USB device, all keys |
| `mac-ioreg-usb-interfaces.plist` | `ioreg -a -l -r -c IOUSBHostInterface -d 1` | XML plist, array of one dict per interface, all keys |
| `mac-ioreg-hid.plist` | `ioreg -a -l -r -c IOHIDDevice -d 1` | XML plist, array of one dict per HID device, only the keys `Transport`, `Built-In`, `VendorID`, `ProductID`, `Product`, `Manufacturer`, `PrimaryUsagePage`, `PrimaryUsage`, `SerialNumber`, `LocationID` (`LocationID` is the key the code joins on) |
| `mac-ioreg-isp.plist` | `ioreg -a -l -r -n isp -d 2` | XML plist, the `isp` node and its children with only `IORegistryEntryName`, `IOObjectClass`, `IOClass`, `IOProviderClass`, `IORegistryEntryChildren` (the device-tree properties and their data blobs are dropped) |
| `linux-sysfs.json` | `collect-sysfs.sh` on the host | `{"dirs": [...], "files": {path: content}, "links": {path: target}}`, all paths relative to the sysfs root |

The ioreg fixtures are plist, not JSON, because the code parses plist and JSON would lose the
difference between integers, booleans and data.

### Rebuilding the sysfs tree

Create every entry of `dirs`, write every entry of `files` (contents are exact, including the
trailing newline sysfs adds), and make every entry of `links` a symbolic link to its target
(create the target directory if it does not exist; a relative link is fine, the code
canonicalises). What is in the tree:

- `bus/usb/devices/<entry>` links to the device or interface directory under `devices/`, which
  holds `idVendor`, `idProduct`, `product`, `manufacturer`, `serial`, `b{Device,Interface}{Class,SubClass,Protocol}`,
  `removable`, `devpath`, `busnum` where the host has them; `port` links to the port directory
  holding `connect_type`.
- `class/{input,video4linux,sound}/<entry>` links to the node directory; its `device` link
  points at the parent (missing for some sound nodes, as on the host). Video nodes have `name`
  and `index`, sound cards `id` and `index`, `inputN` directories `capabilities/{ev,key,rel}`.
- Every ancestor of a class node's parent: `firmware_node` and `of_node` are recorded as empty
  directories when they exist (on the host `firmware_node` is a link; the code only checks
  existence), and `subsystem` as a link to `bus/<name>` or `class/<name>`.

Nothing else from those folders was taken.

## What was kept (schema, not identity)

USB and interface class, subclass and protocol codes; booleans and flags (`Built-In`,
`removable`, `connect_type`, `USBPortType`, speeds); transport kinds (`USB`, `Bluetooth`,
`FIFO`, `SPU`, `Audio`, `PCIe`); IOKit class names and `com.apple.*` identifiers; usage pages
and usages; `ev`/`key`/`rel` capability bitmaps; `coreaudio_*`/`spaudio_*`/`attrib_*` enum
tokens; link speed and power allocation strings; USB hardware types (`Built-in`, `Removable`,
`Non-removable`); Bluetooth minor types and service lists; battery levels and RSSI; sysfs
`devpath`, `busnum`, `index`; kernel path elements (PCI addresses, `usbN`, `1-2.3:1.0`,
`LNX*`/`PNP*` ACPI ids, `inputN`, `eventN`, `pcmCxDyc`, `cardN`, ...).

Also kept: the two Core Audio source labels of the built-in device exactly as macOS shows them
in Japanese (`MacBook Air` + `のマイク` / `のスピーカー`). They are Apple's standard labels,
not a name of the owner's, and they are the real case the code has to handle.

## What was replaced

| What | Placeholder |
|---|---|
| vendor ids | `0xffff`, `0xfffe`, ... (same type: int stays int, `0x05ac`-style string stays a 4-digit hex string, sysfs `idVendor` stays 4 lowercase hex digits) |
| product ids (incl. `idProductArray`, and the ids inside HID path elements like `0003:VVVV:PPPP.0001`) | `0xff00`, `0xfeff`, ... |
| device versions (`bcdDevice`, `USBDeviceKeyProductVersion`) | `0x0100`, `0x0101`, ... |
| location ids (`locationID`, `LocationID`, `USBKeyLocationID`, `IORegistryEntryLocation`) | the bus byte becomes `0x80`, `0x81`, ...; the port nibbles below it are kept so the topology and `port_path` still work |
| serial numbers (all keys with "serial", sysfs `serial`) | `SERIAL-n` (`Not Provided` kept) |
| product, manufacturer, `_name`, audio manufacturer, interface strings, USB device registry names | `Example Product n`, `Example Maker n` |
| Bluetooth device names (they are dict keys) and any other key that is not a plain identifier | `Example Device n`, `Example Key n` |
| hardware addresses (Bluetooth controller and devices) | `02:00:00:00:00:nn` |
| UUIDs (camera unique ids, `kUSBContainerID`, the `IOCFPlugInTypes` keys) | `00000000-0000-4000-8000-00000000000n` |
| every data blob (`UsbDeviceSignature`, `kOSBundleDextUniqueIdentifier`) | made-up bytes of the same length (24 blobs on the Mac; the HID and isp blobs were dropped with their keys) |
| hex strings (code-directory hashes, service masks) | made-up hex of the same length |
| integers of 6 or more digits (`IORegistryEntryID`, `sessionID`, busy times, ...) | `10…0n` with the same number of digits |
| versions, chipset name | made-up of the same shape (a four-part version ends in 900+n so it never looks like an address) |
| camera model ids, audio source labels other than the generic ones, V4L names, sound card ids | `Example Model n`, `Example Source n`, `Example Camera n`, `ExampleCardn` |
| any other string the rules do not know | `Example Value n` |

## Designed strings (made up, added after sanitizing)

- `mac-system_profiler.json`: the first camera is named `Example Toy's Camera` (ASCII apostrophe).
- `mac-ioreg-usb.plist`: the first device that is not a hub has `kUSBProductString` and
  `USB Product Name` `Example Café Camera` (non-ASCII). A control character cannot be put in an
  XML plist.
- `linux-sysfs.json`: the first USB device product below a root hub is
  `Example Toy's Webcamé` followed by a BEL control character (U+0007) and the newline.
- A Bluetooth earphone label (`Example Toy’s Earphones`) is set only when the audio report has a
  Bluetooth device; on this run it had none, so it was not added.

## Checks run on every fixture

- `sanitize.py` refuses to write a fixture that contains the Mac's or the host's host names or
  login names (given in memory through `ISTESSO_SCRUB`), matches the machine's private-words
  file, or holds a data blob that is not one of its own placeholders.
- `scripts/check-private.sh --file` (with the private-words file): exit 0 on all six.
- Counts of MAC, IPv4, UUID shapes, apostrophes, non-ASCII and digit runs of 6 or more: every
  hit is a placeholder or a designed string (see the report).
