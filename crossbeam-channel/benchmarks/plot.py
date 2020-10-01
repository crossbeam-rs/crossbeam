#!/usr/bin/env python3
import random
import sys
import matplotlib.pyplot as plt


def read_data(files):
    results = []
    for f in files:
        with open(f) as f:
            for line in f.readlines():
                test, lang, impl, secs, _ = line.split()
                splt = test.split('_')
                results.append((splt[0], '_'.join(splt[1:]), lang, impl, float(secs)))
    return results


def get_runs(results, prefix):
    runs = set()
    for pre, test, lang, impl, secs in results:
        if pre == prefix:
            runs.add(test)
    result = list(runs)
    result.sort()
    return result


def find(s, x):
    for i in range(len(s)):
        if s[i] == x:
            return i
    return None


color_set = {
    'aqua': '#00ffff',
    'azure': '#f0ffff',
    'beige': '#f5f5dc',
    'black': '#000000',
    'blue': '#0000ff',
    'brown': '#a52a2a',
    'cyan': '#00ffff',
    'darkblue': '#00008b',
    'darkcyan': '#008b8b',
    'darkgrey': '#a9a9a9',
    'darkgreen': '#006400',
    'darkkhaki': '#bdb76b',
    'darkmagenta': '#8b008b',
    'darkolivegreen': '#556b2f',
    'darkorange': '#ff8c00',
    'darkorchid': '#9932cc',
    'darkred': '#8b0000',
    'darksalmon': '#e9967a',
    'darkviolet': '#9400d3',
    'fuchsia': '#ff00ff',
    'gold': '#ffd700',
    'green': '#008000',
    'indigo': '#4b0082',
    'khaki': '#f0e68c',
    'lightblue': '#add8e6',
    'lightcyan': '#e0ffff',
    'lightgreen': '#90ee90',
    'lightgrey': '#d3d3d3',
    'lightpink': '#ffb6c1',
    'lightyellow': '#ffffe0',
    'lime': '#00ff00',
    'magenta': '#ff00ff',
    'maroon': '#800000',
    'navy': '#000080',
    'olive': '#808000',
    'orange': '#ffa500',
    'pink': '#ffc0cb',
    'purple': '#800080',
    'red': '#ff0000',
}
saved_color = {}


def get_color(name):
    if name not in saved_color:
        color = color_set.popitem()
        saved_color[name] = color
    return saved_color[name][1]


def plot(results, fig, subplot, title, prefix):
    runs = get_runs(results, prefix)

    ys = [len(runs) * (i + 1) for i in range(len(runs))]

    ax = fig.add_subplot(subplot)
    ax.set_title(title)
    ax.set_yticks(ys)
    ax.set_yticklabels(runs)
    ax.tick_params(which='major', length=0)
    ax.set_xlabel('seconds')

    scores = {}

    for pre, test, lang, impl, secs in results:
        if pre == prefix:
            name = impl if lang == 'Rust' else impl + f' ({lang})'
            if name not in scores:
                scores[name] = [0] * len(runs)
            scores[name][find(runs, test)] = secs

    opts = dict(height=0.8, align='center')
    x_max = max(max(scores.values(), key=lambda x: max(x)))
    for i, (name, score) in enumerate(scores.items()):
        yy = [y + i - len(runs) // 2 + 0.2 for y in ys]
        ax.barh(yy, score, color=get_color(name), **opts)
        for xxx, yyy in zip(score, yy):
            if xxx:
                ax.text(min(x_max - len(name) * 0.018 * x_max, xxx), yyy - 0.25, name, fontsize=9)


def plot_all(results, descriptions, labels):
    fig = plt.figure(figsize=(10, 10))
    # TODO support more number subplots
    subplot = [221, 222, 223, 224]
    for p, d, l in zip(subplot, descriptions, labels):
        plot(results, fig, p, d, l)
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


def main():
    results = read_data(sys.argv[1:])
    descriptions = [
        'Bounded channel of capacity 0',
        'Bounded channel of capacity 1',
        'Bounded channel of capacity N',
        'Unbounded channel',
    ]
    labels = ['bounded0', 'bounded1', 'bounded', 'unbounded']
    plot_all(results, descriptions, labels)


if __name__ == '__main__':
    main()
