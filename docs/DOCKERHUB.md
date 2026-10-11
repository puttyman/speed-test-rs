# speed-test-rs

A free, self-hosted speed test for your home or office network. Run it on any machine on your LAN, open it from any phone, laptop or TV browser, and see how fast your Wi-Fi and wired connections really are. Nothing leaves your network.

![A speed test in progress](https://raw.githubusercontent.com/puttyman/speed-test-rs/main/docs/screenshots/test-running.png)

## Features

- **Download, upload, latency, jitter and packet loss**, plus latency under load ("bufferbloat") and a rating for streaming, gaming and video calls.
- **See every connected device**, live, with its current download or upload speed.
- **Test together**: a synchronised test on several devices at once, with the total bandwidth and how fairly it was shared.
- **History and compare**: every run is saved under a label like "Kitchen" or "Office desk".
- **Small and private**: one static binary in a distroless image, runs as non-root, no accounts, no cloud, no external fonts or CDN.

![Devices panel showing other devices' live speeds](https://raw.githubusercontent.com/puttyman/speed-test-rs/main/docs/screenshots/devices.png)

## Quick start

### Docker Compose

```yaml
services:
  speed-test:
    image: puttyman/speed-test-rs:latest
    restart: unless-stopped
    ports:
      - "8080:8080/tcp"
      - "3478:3478/udp"   # packet-loss test (WebRTC)
    environment:
      SPEEDTEST_PUBLIC_IP: 192.168.1.10   # this host's LAN IP, for the packet-loss test
    volumes:
      - speedtest-data:/data

volumes:
  speedtest-data:
```

Then open `http://<host-ip>:8080` on any device on your network.

### Host networking (Linux, best for multi-gigabit)

No port mapping or public IP needed, and no Docker NAT in the data path:

```bash
docker run -d --name speed-test --network host \
  -v speedtest-data:/data puttyman/speed-test-rs:latest
```

### docker run (bridge)

```bash
docker run -d --name speed-test -p 8080:8080 -p 3478:3478/udp \
  -e SPEEDTEST_PUBLIC_IP=192.168.1.10 -v speedtest-data:/data \
  puttyman/speed-test-rs:latest
```

## Configuration

| Variable | Default | |
|---|---|---|
| `SPEEDTEST_LISTEN` | `0.0.0.0:8080` | HTTP/WebSocket listen address |
| `SPEEDTEST_UDP_PORT` | `3478` | WebRTC port for the packet-loss test. Publish it with the same number on both sides. |
| `SPEEDTEST_PUBLIC_IP` | auto | Host LAN IP advertised for the packet-loss test. **Set it with bridge networking**, otherwise packet loss shows as unavailable. |
| `SPEEDTEST_SERVER_NAME` | hostname | Name shown in the UI |
| `SPEEDTEST_HISTORY` | `true` | `false` turns off saving runs |
| `SPEEDTEST_RETENTION_DAYS` | keep all | Delete runs older than this |
| `SPEEDTEST_PRESENCE` | `true` | `false` turns off the devices panel and group tests |
| `SPEEDTEST_AUTH_TOKEN` | none | Require a token; set it before exposing beyond a trusted LAN |

History is a SQLite database in the `/data` volume.

## Image details

- **Tags**: `latest`, `X.Y.Z`, `X.Y`
- **Platforms**: `linux/amd64`, `linux/arm64` (Raspberry Pi 4/5 with a 64-bit OS)
- **Ports**: `8080/tcp`, `3478/udp`
- **Volume**: `/data`
- **Base**: `gcr.io/distroless/static-debian12:nonroot`, with a built-in `HEALTHCHECK`

## Tips

- Run it on a **wired** machine: it measures the link between your device and that machine.
- Docker's userland proxy can cap throughput on slow hosts; use host networking for the most accurate results.

## Links

- Source, binaries for Linux/macOS/Windows, full guide: https://github.com/puttyman/speed-test-rs
- License: MIT or Apache-2.0
