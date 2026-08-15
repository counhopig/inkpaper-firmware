#!/usr/bin/env python3
"""Generate an NVS image holding the Wi-Fi credentials and flash it into the
`nvs` partition of a NOTE4.

Usage:
    ./scripts/gen-nvs-wifi.py --ssid MyWiFi --password secret [--port /dev/ttyACM0]

The image only contains the `wifi_ssid` / `wifi_pass` keys in the `inkpaper`
namespace. Writing it REPLACES the whole NVS partition, so any other stored
data (e.g. the button counters) is reset. Run once at provisioning time.

Requires an activated ESP-IDF environment (`get_idf`) for `nvs_partition_gen.py`
and `parttool.py`, or pass --idf-path to point at the ESP-IDF tree.
"""

import argparse
import os
import subprocess
import sys
import tempfile

NAMESPACE = "inkpaper"
NVS_PARTITION_SIZE = 0x6000


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssid", required=True, help="Wi-Fi network name")
    parser.add_argument("--password", default="", help="Wi-Fi passphrase (empty = open network)")
    parser.add_argument("--port", default="/dev/ttyACM0", help="serial port of the NOTE4")
    parser.add_argument("--idf-path", default=os.environ.get("IDF_PATH", ""), help="ESP-IDF root")
    args = parser.parse_args()

    if not args.idf_path:
        print("error: ESP-IDF not found. Run `get_idf` first or pass --idf-path.", file=sys.stderr)
        return 1

    gen = os.path.join(args.idf_path, "components", "nvs_flash", "nvs_partition_generator", "nvs_partition_gen.py")
    parttool = os.path.join(args.idf_path, "components", "partition_table", "parttool.py")

    with tempfile.TemporaryDirectory() as tmp:
        csv = os.path.join(tmp, "wifi.csv")
        img = os.path.join(tmp, "wifi-nvs.bin")
        with open(csv, "w", encoding="utf-8") as f:
            f.write("key,type,encoding,value\n")
            f.write(f"{NAMESPACE},namespace,,\n")
            f.write(f"wifi_ssid,data,string,{args.ssid}\n")
            f.write(f"wifi_pass,data,string,{args.password}\n")

        print(f"generating {img} with SSID '{args.ssid}' (password: {'set' if args.password else 'none'})")
        subprocess.run(
            [sys.executable, gen, "generate", csv, img, hex(NVS_PARTITION_SIZE)],
            check=True,
        )
        print(f"flashing NVS image to {args.port}")
        subprocess.run(
            [
                sys.executable,
                parttool,
                "--port",
                args.port,
                "write_partition",
                "--partition-name=nvs",
                "--input",
                img,
            ],
            check=True,
        )
    print("done. Reboot the NOTE4 (CTRL+R in espflash monitor) to connect.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
