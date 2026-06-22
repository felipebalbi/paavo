# contrib

Deployment assets. paavo does **not** install these for you.

## Install

```bash
sudo install -d /etc/paavo /var/lib/paavo
sudo install -m 0644 paavo.toml.example /etc/paavo/paavo.toml   # then edit
# /home/paavo is a real login home for SSH maintenance; the daemon's data
# still lives in the state_dir (/var/lib/paavo, owned via StateDirectory=).
sudo useradd --system --create-home --home-dir /home/paavo --shell /bin/bash paavo
sudo chown paavo:paavo /var/lib/paavo                          # StateDirectory= also enforces this on start
sudo install -m 0755 ../target/release/paavod    /usr/local/bin/
sudo install -m 0755 ../target/release/paavo-web /usr/local/bin/
sudo install -m 0644 paavod.service    /etc/systemd/system/
sudo install -m 0644 paavo-web.service /etc/systemd/system/
sudo install -m 0644 99-probes.rules   /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
sudo systemctl daemon-reload
sudo systemctl enable --now paavod.service paavo-web.service
```
