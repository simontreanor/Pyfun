# Hand-written Python baseline for collatz.pyfun: the loop a Pythonista would
# write (iterative, no recursion). Must print exactly what the compiled Pyfun
# version prints.


def collatz(n):
    steps = 0
    while n != 1:
        if n % 2 == 0:
            n //= 2
        else:
            n = 3 * n + 1
        steps += 1
    return steps


def main():
    total = sum(collatz(n) for n in range(1, 120000))
    print(f"collatz checksum: {total}")


main()
