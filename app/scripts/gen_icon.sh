#!/usr/bin/env bash
# Generate the MixID app icon: 1024x1024, dark bg (#0e1116), orange (#ff5500)
# waveform motif — vertical rounded bars of varying heights, centered, subtle
# glow, muted baseline. Matches the web UI palette (static/index.html).
#
# Regenerate: bash app/scripts/gen_icon.sh   (writes app/icon.png)
# (PIL is broken in local python3.14, so ImageMagick is used.)
set -euo pipefail
OUT="/home/fivelidz/projects/GLM_projects/mix_identifier/app/icon.png"

# Bar geometry — deterministic heights (same envelope+jitter as the original
# PIL design, seed 7): n=15 bars, bar_w=34, gap=22, max_h=560, cy=512.
DRAW_ARGS=()
while IFS= read -r line; do
  DRAW_ARGS+=(-draw "$line")
done < <(python3 - <<'EOF'
import random
random.seed(7)
n_bars, bar_w, gap, max_h, cy = 15, 34, 22, 560, 512
total_w = n_bars * bar_w + (n_bars - 1) * gap
x0 = (1024 - total_w) // 2
for i in range(n_bars):
    t = abs(i - (n_bars - 1) / 2) / ((n_bars - 1) / 2)
    env = 1.0 - 0.72 * t
    jitter = 0.55 + random.random() * 0.45
    h = max_h * env * jitter
    x = x0 + i * (bar_w + gap)
    top, bot = cy - h / 2, cy + h / 2
    color = "#ff7a33" if i in (6, 7, 8) else "#ff5500"
    r = bar_w / 2
    print(f"fill {color} roundrectangle {x:.0f},{top:.0f} {x+bar_w:.0f},{bot:.0f} {r:.0f},{r:.0f}")
# baseline scan line under the waveform (muted #8b93a1)
line_y = cy + max_h // 2 + 70
print(f"fill '#8b93a1' roundrectangle {x0},{line_y} {x0+total_w},{line_y+10} 5,5")
EOF
)

magick -size 1024x1024 xc:'#0e1116' \
  \( -size 1024x1024 xc:none -fill 'rgba(255,85,0,0.16)' \
     -draw 'ellipse 512,512 330,260 0,360' -gaussian-blur 60 \) \
  -composite \
  -stroke none "${DRAW_ARGS[@]}" \
  "$OUT"
echo "wrote $OUT"
magick identify "$OUT"
