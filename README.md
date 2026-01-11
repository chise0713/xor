# xor

## Usage
```console
usage: xor [-h] [-b buffer_limit_usize] [-l listen_address] [-m mtu_usize] [-r remote_address] [-t token_hex_u8]
```
## Example
local:
```console
xor -b2048 -l127.0.0.1:51820 -m1452 -r place.holder.local.arpa:65535 -t0xFF
```
remote:
```console
xor -b2048 -l[::]:65535 -m1452 -r127.0.0.1:51820 -t0xFF
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