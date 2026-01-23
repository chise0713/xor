#!/bin/sh

set -eu

while IFS= read -r cmd; do
	case "$cmd" in
	'' | \#*)
		continue
		;;
	esac
	command -v "$cmd" >/dev/null || {
		printf '%s not found\n' "$cmd" >&2
		exit 127
	}
done <<"EOF"

# build and observe tools
cargo
htop
pv
socat
tmux

# base utils
cat
cp
dirname
mktemp
rm
sed

EOF

SCRIPT_DIR=$(
	CDPATH= cd -- "$(dirname -- "$0")" && pwd
)

SESSION="udp-bench"

HTOP_TEMP_DIR="$(mktemp -d)"
mkdir -p "$HTOP_TEMP_DIR/htop"
cleanup() {
	[ -d "$HTOP_TEMP_DIR" ] && rm -rf "$HTOP_TEMP_DIR"
}
trap cleanup EXIT

cp "$HOME/.config/htop/htoprc" "$HTOP_TEMP_DIR/htop/htoprc"

cat <<EOF >"$HTOP_TEMP_DIR/htop/htoprc"
config_reader_min_version=3
fields=48 0 18 39 40 46 47 49 53 1
highlight_base_name=1
highlight_deleted_exe=1
shadow_distribution_path_prefix=0
highlight_megabytes=1
highlight_threads=1
find_comm_in_cmdline=1
show_merged_command=1
show_thread_names=1
show_program_path=1
color_scheme=0
enable_mouse=1
delay=15
hide_function_bar=2
header_layout=one_100
column_meters_0=!
column_meter_modes_0=!
sort_key=46
sort_direction=-1
screen:Main=PID M_VIRT M_RESIDENT M_SHARE STATE PERCENT_CPU PERCENT_MEM TIME ELAPSED Command
.sort_key=PERCENT_CPU
.tree_sort_key=PERCENT_CPU
.tree_view_always_by_pid=0
.tree_view=0
.sort_direction=-1
.tree_sort_direction=-1
.all_branches_collapsed=0
EOF

if tmux has-session -t "$SESSION" 2>/dev/null; then
	tmux kill-session -t "$SESSION" 2>/dev/null
fi
buf_size=65487
method="xor"
io_uring=0
while getopts "b:s:i" opt; do
	case "$opt" in
	b)
		case "$OPTARG" in
		*[!0-9]* | "")
			buf_size=65487
			;;
		*)
			buf_size="$OPTARG"
			;;
		esac
		;;
	s)
		case "$OPTARG" in
		xor | dns)
			method="$OPTARG"
			;;
		*)
			method="xor"
			;;
		esac
		;;
	i)
		io_uring=1
		;;
	esac
done
method_opt="-sxor"
mtu_size=$((buf_size + 48))

if [ "$method" = "dns" ]; then
	method_opt="-sdnspad"
	buf_size=$((buf_size - 56))
fi

(
	cd "$SCRIPT_DIR" || exit 1
	EXTRA_ARGS=""
	if [ "$io_uring" -eq 1 ]; then
		export RUSTFLAGS="--cfg tokio_unstable"
		EXTRA_ARGS="--features io-uring"
	fi
	cargo build --release $EXTRA_ARGS
)
sleep 0.025
TARGET_DIR=$(
	cd "$SCRIPT_DIR" &&
		cargo metadata --format-version 1 --no-deps |
		sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'
)

tmux new-session -d -s "$SESSION" \
	"exec socat -u -b $buf_size UDP-RECV:12345,bind=127.0.0.2,rcvbuf=33554432 \
            EXEC:\"/bin/pv -B$buf_size -k -8 -b -t -r -p -X\""

tmux set-hook -t "$SESSION" client-detached \
	"if -F '#{==:#{session_attached},0}' 'kill-session -t $SESSION'"

tmux set-option -g prefix None
tmux set-option status off
tmux bind-key -n q kill-session

tmux new-window -d -t "$SESSION" \
	"exec socat -b $buf_size /dev/zero UDP-SENDTO:127.0.0.1:12345,sndbuf=33554432"

tmux select-window -t "$SESSION":0

tmux split-window -v \
	"sh -c \"\"$TARGET_DIR\"/release/xor \
            -l127.0.0.1:12345 -r127.0.0.2:12345 -t0x30 -o5 -m$mtu_size $method_opt; sleep infinity\""

tmux split-window -v \
	"exec env XDG_CONFIG_HOME=\"$HTOP_TEMP_DIR\" htop \
            -p \"\$(pidof xor)\" \
            -p \"\$(pidof xor socat | tr ' ' ',')\"\
            -p \"\$(pidof socat pv | tr ' ' ',')\""

tmux set-hook -t "$SESSION" -a client-resized \
	"resize-pane -t 0.0 -y 1"

tmux set-hook -t "$SESSION" -a client-resized \
	"resize-pane -t 0.1 -y 3"

tmux select-pane -t "$SESSION":0.1

tmux attach-session -t "$SESSION"