import os
import math
from typing import List, Optional, Set, Tuple

import torch
from torch import nn

from text_correction_utils.api import utils


def generate_table(
    headers: List[List[str]],
    data: List[List[str]],
    alignments: Optional[List[str]] = None,
    horizontal_lines: Optional[List[int]] = None,
    mark_bold: Optional[Set[Tuple[int, int]]] = None,
    max_column_width: int = 48
) -> str:
    assert len(headers), "got no headers"
    assert len(set(len(header) for header in headers)) == 1, "all headers must have the same length"
    header_length = len(headers[0])

    assert all(header_length == len(item) for item in data), \
        f"header has length {header_length}, but data items have lengths {[len(item) for item in data]}"

    if alignments is None:
        alignments = ["left"] + ["right"] * (header_length - 1)

    if mark_bold is None:
        mark_bold = set()

    max_column_width = max(3, max_column_width)

    # get max width for each column in headers and data
    column_widths = []
    for i in range(header_length):
        # add 4 to width if cell is bold because of the two **s left and right
        header_width = max(len(h[i]) + (4 * ((i, j) in mark_bold)) for j, h in enumerate(headers))
        data_width = max(min(max_column_width, len(d[i]) + (4 * ((i, j) in mark_bold))) for j, d in enumerate(data))
        column_widths.append(
            min(
                max_column_width,
                max(
                    # markdown needs at least three - for a proper horizontal line
                    3,
                    header_width,
                    data_width
                )
            )
        )

    if horizontal_lines is None:
        horizontal_lines = [0] * len(data)

    bold_cells = [
        [(i, j) in mark_bold for j in range(len(data[i]))]
        for i in range(len(data))
    ]

    tables_lines = []

    tables_lines.extend([
        _table_row(header, [False] * header_length, alignments, column_widths, max_column_width)
        + (_table_horizontal_line(column_widths) if i == len(headers) - 1 else "")
        for i, header in enumerate(headers)
    ])

    for item, horizontal_line, bold in zip(data, horizontal_lines, bold_cells):
        line = _table_row(item, bold, alignments, column_widths, max_column_width)
        if horizontal_line > 0:
            line += _table_horizontal_line(column_widths)
        tables_lines.append(line)

    return "\n".join(tables_lines)


def _table_cell(s: str, prefix: str, suffix: str, alignment: str, width: int) -> str:
    s = prefix + s + suffix
    if alignment == "left":
        s = s.ljust(width)
    elif alignment == "right":
        s = s.rjust(width)
    else:
        s = s.center(width)
    return s


def _table_row(data: List[str], bold: List[bool], alignments: List[str], widths: List[int], max_width: int) -> str:
    assert len(data) == len(bold)
    num_lines = [math.ceil(len(d) / max_width) for d in data]
    max_num_lines = max(num_lines)
    lines = []
    for i in range(max_num_lines):
        line_data = [d[i*max_width: (i + 1) * max_width] for d in data]
        line = "| " + " | ".join(_table_cell(
            d,
            "**" if b and i == 0 else "",
            "**" if b and i == max_num_lines - 1 else "",
            a,
            w
        ) for d, b, a, w in zip(line_data, bold, alignments, widths)) + " |"
        lines.append(line)
    return "\n".join(lines)


def _table_horizontal_line(widths: List[int]) -> str:
    return "\n| " + " | ".join("-" * w for w in widths) + " |"


def generate_report(
        task: str,
        model_name: str,
        model: nn.Module,
        input_size: int,
        input_size_bytes: int,
        runtime: float,
        precision: torch.dtype,
        batch_size: int,
        sort_by_length: bool,
        device: torch.device,
        file_path: Optional[str] = None
) -> Optional[str]:
    if precision == torch.float16:
        precision_str = "fp16"
    elif precision == torch.bfloat16:
        precision_str = "bfp16"
    elif precision == torch.float32:
        precision_str = "fp32"
    else:
        raise ValueError("expected precision to be one of torch.float16, torch.bfloat16 or torch.float32")

    report = generate_table(
        headers=[["REPORT", task]],
        data=[
            ["Model", model_name],
            ["Input size 1", f"{input_size} sequences"],
            ["Input size 2", f"{input_size_bytes / 1000:,.2f} kB"],
            ["Runtime", f"{runtime:,.1f} s"],
            ["Throughput 1", f"{input_size / runtime:,.1f} seq/s"],
            ["Throughput 2", f"{input_size_bytes / runtime / 1000:,.1f} kB/s"],
            ["GPU memory", f"{torch.cuda.max_memory_reserved(device) // 1024 ** 2:,} MiB"],
            ["Parameters", f"{utils.num_parameters(model)['total'] / 1000 ** 2:,.1f} M"],
            ["Precision", precision_str],
            ["Batch size", f"{batch_size:,}"],
            ["Sorted", "yes" if sort_by_length else "no"],
            ["Device",  f"{utils.cpu_info()}{', ' + utils.device_info(device) if device.type == 'cuda' else ''}"],
        ],
    )
    if file_path is not None:
        if os.path.dirname(file_path):
            os.makedirs(os.path.dirname(file_path), exist_ok=True)

        with open(file_path, "w", encoding="utf8") as of:
            of.write(report + "\n")

        return None
    else:
        return report
