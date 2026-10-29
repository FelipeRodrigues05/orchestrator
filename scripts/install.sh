#!/bin/sh
# Builds the app in release mode and installs it for the current user:
# binary in ~/.local/bin, icon in the hicolor theme, launcher in the app menu.
# Pass cargo features through, e.g. `scripts/install.sh --features gpu-vulkan`.
# `scripts/install.sh --uninstall` removes all three.
set -eu

id=dev.feliperodrigues.Orchestrator
root=$(cd "$(dirname "$0")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin/orchestrator
icon=$data/icons/hicolor/scalable/apps/$id.svg
desktop=$data/applications/$id.desktop

refresh() {
    gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
    update-desktop-database -q "$data/applications" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin" "$icon" "${icon%.svg}-attention.svg" "$desktop"
    refresh
    echo "removed $bin, $icon, $desktop"
    exit 0
fi

cargo build --release -p app --manifest-path "$root/Cargo.toml" "$@"
install -Dm755 "$root/target/release/orchestrator" "$bin"
install -Dm644 "$root/app/data/$id.svg" "$icon"
install -Dm644 "$root/app/data/$id-attention.svg" "${icon%.svg}-attention.svg"
install -Dm644 "$root/app/data/$id.desktop" "$desktop"
# Absolute Exec: launchers do not always have ~/.local/bin on PATH.
sed -i "s|^Exec=.*|Exec=$bin|" "$desktop"
refresh
echo "installed: $bin"
echo "open it from the app launcher (\"Orchestrator\") or run: orchestrator"
