# Hand-written Python baseline for map_build.pyfun: a plain dict built in a
# loop, then the same lookups. Must print exactly what the compiled Pyfun
# version prints.


def key(i):
    return f"key{(i * 1103515245 + 12345) % 2147483648 % 20011}"


def main():
    d = {}
    for i in range(500000):
        d[key(i)] = i

    total = 0
    for i in range(500000):
        total += d.get(key(i * 3), 0)

    print(f"map_build size: {len(d)} checksum: {total}")


main()
