#!/usr/bin/env python3

import sys
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches

results = []
for f in sys.argv[1:]:
    with open(f) as f:
        for line in f.readlines():
            test, lang, impl, secs, _ = line.split()
            results.append((test, lang, impl, float(secs)))

fig = plt.figure(figsize=(10, 10))


def plot(subplot, title, prefix, runs):
    runs.reverse()

    ys = [6 * (i + 1) for i in range(len(runs))]
    ax = fig.add_subplot(subplot)
    ax.set_title(title)
    ax.set_yticks(ys)
    ax.set_yticklabels(runs)
    ax.tick_params(which='major', length=0)
    ax.set_xlabel('seconds')

    scores = {}

    for (i, run) in enumerate(runs):
        for (test, lang, impl, secs) in results:
            if test == prefix + '_' + run:
                name = lang + '_' + impl
                if name not in scores:
                    scores[name] = [0] * len(runs)
                scores[name][i] = secs

    opts = dict(height=0.7, align='center')
    for (i, score) in enumerate(scores.values()):
        ax.barh([y + i - len(scores) // 2 for y in ys], score, **opts)

    m = int(max(max([x for x in scores.values()])) * 1.3)
    if m < 10:
        ax.set_xticks(list(range(m + 1)))
    elif m < 50:
        ax.set_xticks([x * 5 for x in range(m // 5 + 1)])
    elif m < 100:
        ax.set_xticks([x * 10 for x in range(m // 10 + 1)])
    elif m < 400:
        ax.set_xticks([x * 20 for x in range(m // 20 + 1)])
    else:
        ax.set_xticks([x * 100 for x in range(m // 100 + 1)])

    for (i, (name, score)) in enumerate(scores.items()):
        for (x, y) in zip(score, ys):
            ax.text(x + m / 200., y + i - len(scores) // 2 - 0.25, name, fontsize=9)


plot(
    221,
    "Bounded channel of capacity 0",
    'bounded0',
    ['spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
    222,
    "Bounded channel of capacity 1",
    'bounded1',
    ['spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
    223,
    "Bounded channel of capacity N",
    'bounded',
    ['seq', 'spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
    224,
    "Unbounded channel",
    'unbounded',
    ['seq', 'spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plt.subplots_adjust(
    top=0.95,
    bottom=0.05,
    left=0.1,
    right=0.95,
    wspace=0.3,
    hspace=0.2,
)
plt.savefig('plot.png')
# plt.show()
