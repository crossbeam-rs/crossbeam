#!/usr/bin/env python2

import sys

lines = []
for f in sys.argv[1:]:
    with open(f) as f:
        lines += f.readlines()
lines.sort()

last = None
for line in lines:
    test = line.split(' ')[0]
    if test != last:
        if last:
            print
        last = test
    print line,
