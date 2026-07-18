# Hand-written Python baseline for expr_eval.pyfun. This is the ceiling
# reference: the program a Pythonista would write for the same job, using
# tagged tuples and match statements. Must print exactly what the compiled
# Pyfun version prints.

M = 1000003


def next_seed(s):
    return (s * 1103515245 + 12345) % 2147483648


def build(depth, seed):
    if depth == 0:
        return ("num", seed % 100)
    s1 = next_seed(seed)
    s2 = next_seed(s1)
    r = seed % 4
    if r == 0:
        return ("neg", build(depth - 1, s1))
    if r == 1:
        return ("mul", build(depth - 1, s1), build(depth - 1, s2))
    return ("add", build(depth - 1, s1), build(depth - 1, s2))


def evaluate(e):
    match e:
        case ("num", n):
            return n
        case ("add", a, b):
            return (evaluate(a) + evaluate(b)) % M
        case ("mul", a, b):
            return (evaluate(a) * evaluate(b)) % M
        case ("neg", a):
            return -evaluate(a)


def simplify(e):
    match e:
        case ("num", _):
            return e
        case ("neg", a):
            sa = simplify(a)
            if sa[0] == "num":
                return ("num", -sa[1])
            return ("neg", sa)
        case ("add", a, b):
            sa, sb = simplify(a), simplify(b)
            match (sa, sb):
                case (("num", 0), _):
                    return sb
                case (_, ("num", 0)):
                    return sa
                case (("num", x), ("num", y)):
                    return ("num", (x + y) % M)
                case _:
                    return ("add", sa, sb)
        case ("mul", a, b):
            sa, sb = simplify(a), simplify(b)
            match (sa, sb):
                case (("num", 0), _) | (_, ("num", 0)):
                    return ("num", 0)
                case (("num", 1), _):
                    return sb
                case (_, ("num", 1)):
                    return sa
                case (("num", x), ("num", y)):
                    return ("num", (x * y) % M)
                case _:
                    return ("mul", sa, sb)


def main():
    acc = 0
    for i in range(400):
        tree = build(12, next_seed(i * 7919))
        v1 = evaluate(tree)
        v2 = evaluate(simplify(tree))
        acc = (acc * 31 + v1 * 17 + v2) % 1000000007
    print(f"expr_eval checksum: {acc}")


main()
