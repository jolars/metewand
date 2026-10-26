"""A black-box solver with an iteration limit and no observation callbacks."""


def bisect(iterations):
    lower, upper = 1.0, 2.0
    for _ in range(iterations):
        midpoint = (lower + upper) / 2
        if midpoint * midpoint < 2:
            lower = midpoint
        else:
            upper = midpoint
    return (lower + upper) / 2
