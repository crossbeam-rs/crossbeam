#!/usr/bin/env python3

from __future__ import annotations

import sys
from dataclasses import dataclass
from math import ceil
from pathlib import Path

import plotly.graph_objects as go
from plotly.subplots import make_subplots

COLUMN_LENGTH = 2


Pre = str
Test = str
Lang = str
Impl = str
Secs = float
BenchResult = tuple[Pre, Test, Lang, Impl, Secs]


@dataclass
class Label:
    name: Pre
    description: str


def read_data(files: list[str]) -> list[BenchResult]:
    results: list[BenchResult] = []
    for file in files:
        with Path(file).open() as f:
            for line in f:
                temp, lang, impl, secs, _ = line.split()
                pre, test = temp.split("_", maxsplit=1)
                results.append(
                    (Pre(pre), Test(test), Lang(lang), Impl(impl), Secs(secs)),
                )
    return results


def get_scores(results: list[BenchResult], label: Label) -> dict[str, dict[Test, Secs]]:
    scores: dict[str, dict[Test, Secs]] = {}
    for pre, test, lang, impl, secs in results:
        if pre != label.name:
            continue
        name = impl if lang == "Rust" else f"{impl} ({lang})"
        scores.setdefault(name, {})[test] = secs
    return scores


color_set: dict[str, str] = {
    "aqua": "#00ffff",
    "azure": "#f0ffff",
    "beige": "#f5f5dc",
    "black": "#000000",
    "blue": "#0000ff",
    "brown": "#a52a2a",
    "cyan": "#00ffff",
    "darkblue": "#00008b",
    "darkcyan": "#008b8b",
    "darkgrey": "#a9a9a9",
    "darkgreen": "#006400",
    "darkkhaki": "#bdb76b",
    "darkmagenta": "#8b008b",
    "darkolivegreen": "#556b2f",
    "darkorange": "#ff8c00",
    "darkorchid": "#9932cc",
    "darkred": "#8b0000",
    "darksalmon": "#e9967a",
    "darkviolet": "#9400d3",
    "fuchsia": "#ff00ff",
    "gold": "#ffd700",
    "green": "#008000",
    "indigo": "#4b0082",
    "khaki": "#f0e68c",
    "lightblue": "#add8e6",
    "lightcyan": "#e0ffff",
    "lightgreen": "#90ee90",
    "lightgrey": "#d3d3d3",
    "lightpink": "#ffb6c1",
    "lightyellow": "#ffffe0",
    "lime": "#00ff00",
    "magenta": "#ff00ff",
    "maroon": "#800000",
    "navy": "#000080",
    "olive": "#808000",
    "orange": "#ffa500",
    "pink": "#ffc0cb",
    "purple": "#800080",
    "red": "#ff0000",
}
saved_color: dict[str, tuple[str, str]] = {}


def get_color(name: str) -> str:
    if name not in saved_color:
        color = color_set.popitem()
        saved_color[name] = color
    return saved_color[name][1]


def plot(
    scores: dict[str, dict[Test, Secs]],
    fig: go.Figure,
    row: int,
    column: int,
) -> None:
    for key, value in scores.items():
        tests: list[Test] = []
        secs: list[Secs] = []
        for inner_key, inner_value in value.items():
            tests.append(inner_key)
            secs.append(inner_value)
        fig.add_trace(
            go.Bar(
                name=key,
                x=secs,
                y=tests,
                marker_color=get_color(key),
                orientation="h",
                text=key,
                constraintext="none",
                textposition="auto",
            ),
            row=row,
            col=column,
        )


def plot_all(results: list[BenchResult], labels: list[Label]) -> None:
    rows = ceil(len(labels) / COLUMN_LENGTH)
    titles = [i.description for i in labels]
    fig = make_subplots(
        rows=rows,
        cols=COLUMN_LENGTH,
        subplot_titles=titles,
        horizontal_spacing=0.1,
        vertical_spacing=0.1,
    )
    max_length = 0
    for i, label in enumerate(labels):
        row, column = divmod(i, COLUMN_LENGTH)
        (row, column) = (row + 1, column + 1)
        scores: dict[str, dict[Test, Secs]] = get_scores(results, label)
        max_length = max(max_length, len(scores))
        plot(scores, fig, row, column)
        fig.update_xaxes(title_text="seconds", row=row, col=column)
    fig.update_layout(
        showlegend=False,
        barmode="group",
        width=COLUMN_LENGTH * 1024,
        height=rows * max_length * 128,
    )
    fig.update_yaxes(categoryorder="category ascending")
    fig.write_image("plot.png")


def main() -> None:
    labels: list[Label] = [
        Label(Pre("bounded0"), description="Bounded channel of capacity 0"),
        Label(Pre("bounded1"), description="Bounded channel of capacity 1"),
        Label(Pre("bounded"), description="Bounded channel of capacity N"),
        Label(Pre("unbounded"), description="Unbounded channel"),
    ]
    results: list[BenchResult] = read_data(sys.argv[1:])
    plot_all(results, labels)


if __name__ == "__main__":
    main()
