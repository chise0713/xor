# xor

## Usage
```console
usage: xor [-h] [-l listen_address] [-m mtu_usize]
           [-o timeout_f64_secs] [-r remote_address]
           [-s set_method] [-t token_hex_u8]
Command Summary:
        -h              prints this help message
        -l              listen address
        -m              for link mtu
        -o              client timeout in seconds
        -r              remote address
        -s              one of: `xor`, `dnspad`, `dnsunpad`; defaults to `xor`
        -t              e.g. 0xFF
```
## Example
### XOR
- local:
```console
xor -l127.0.0.1:51820 -m1492 -o2 -r place.holder.local.arpa:65535 -t0xFF
```
- remote:
```console
xor -l[::]:65535 -m1492 -o2 -r127.0.0.1:51820 -t0xFF
```

### Padding DNS Query
- local:
```console
xor -l127.0.0.1:51820 -m1492 -o2 -r place.holder.local.arpa:65535 -sdnspad
```
- remote:
```console
xor -l[::]:65535 -m1492 -o2 -r127.0.0.1:51820 -sdnsunpad
```

## License
```text
Copyright (C) 2026 chise0713

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
```