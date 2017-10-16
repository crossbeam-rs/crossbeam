#!/usr/bin/env python2

import sys
import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
import plotly.plotly as py

results = []
for f in sys.argv[1:]:
    with open(f) as f:
        for line in f.readlines():
            test, lang, impl, secs, _ = line.split()
            results.append((test, lang, impl, float(secs)))

fig = plt.figure(figsize=(10, 10))

def plot(subplot, title, prefix, runs):
    runs.reverse()

    ys = [7 * (i+1) for i in xrange(len(runs))]
    ax = fig.add_subplot(subplot)
    ax.set_title(title)
    ax.set_yticks(ys)
    ax.set_yticklabels(runs)
    ax.tick_params(which='major', length=0)
    ax.set_xlabel('seconds')

    go = [0] * len(runs)
    mpsc = [0] * len(runs)
    msqueue = [0] * len(runs)
    segqueue = [0] * len(runs)
    chan = [0] * len(runs)
    channel = [0] * len(runs)

    for (i, run) in enumerate(runs):
        for (test, lang, impl, secs) in results:
            if test == prefix + '_' + run:
                if lang == 'Go' and impl == 'chan':
                    go[i] = secs
                if lang == 'Rust' and impl == 'mpsc':
                    mpsc[i] = secs
                if lang == 'Rust' and impl == 'MsQueue':
                    msqueue[i] = secs
                if lang == 'Rust' and impl == 'SegQueue':
                    segqueue[i] = secs
                if lang == 'Rust' and impl == 'chan':
                    chan[i] = secs
                if lang == 'Rust' and impl == 'channel':
                    channel[i] = secs

    h = ax.barh([y-3 for y in ys], go, height=0.9, color='skyblue', align='center')
    h = ax.barh([y-2 for y in ys], channel, height=0.9, color='red', align='center')
    h = ax.barh([y-1 for y in ys], mpsc, height=0.9, color='black', align='center')
    h = ax.barh([y+0 for y in ys], chan, height=0.9, color='orange', align='center')
    h = ax.barh([y+1 for y in ys], msqueue, height=0.9, color='blue', align='center')
    h = ax.barh([y+2 for y in ys], segqueue, height=0.9, color='green', align='center')

    m = int(max(go + mpsc + msqueue + segqueue + chan + channel) * 1.1 + 1)
    if m < 10:
        ax.set_xticks(range(m + 1))
    elif m < 50:
        ax.set_xticks([x*5 for x in range(m / 5 + 1)])
    else:
        ax.set_xticks([x*10 for x in range(m / 10 + 1)])

plot(
   221,
   "Bounded channel with capacity 0",
   'bounded0',
   ['spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
   222,
   "Bounded channel with capacity 1",
   'bounded1',
   ['spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
   223,
   "Bounded channel with capacity N",
   'bounded',
   ['seq', 'spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

plot(
   224,
   "Unbounded channel",
   'unbounded',
   ['seq', 'spsc', 'mpsc', 'mpmc', 'select_rx', 'select_both'],
)

legend = [
    ('Go channel', 'skyblue'),
    ('crossbeam-channel', 'red'),
    ('std::sync::mpsc', 'black'),
    ('chan', 'orange'),
    ('crossbeam::sync::MsQueue', 'blue'),
    ('crossbeam::sync::SegQueue', 'green'),
]
legend.reverse()
fig.legend(
    [mpatches.Patch(label=label, color=color) for (label, color) in legend],
    [label for (label, _) in legend],
    'upper center',
    ncol=2,
)

plt.subplots_adjust(top=0.88, bottom=0.05, wspace=0.3, hspace=0.2, left=0.1, right=0.95)
plt.savefig('plot.png')
# plt.show()
