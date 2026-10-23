"""Small reference algorithms; these are not adapters to the research packages."""


def soft(value, penalty):
    return max(value - penalty, 0) - max(-value - penalty, 0)


def pool(values):
    blocks = []
    for value in values:
        blocks.append([value, 1])
        while (
            len(blocks) > 1
            and blocks[-2][0] / blocks[-2][1] > blocks[-1][0] / blocks[-1][1]
        ):
            total, count = blocks.pop()
            blocks[-1][0] += total
            blocks[-1][1] += count
    return [total / count for total, count in blocks for _ in range(count)]


def solve(data):
    y = data["y"]
    family = data["family"]
    if family == "intercepts":
        x = data["x"]
        means = [sum(col) / len(x) for col in zip(*x)]
        my = sum(y) / len(y)
        columns = [[row[j] - means[j] for row in x] for j in range(len(means))]
        # The extracted design has orthogonal centered columns.
        beta = [
            soft(sum(a * (b - my) for a, b in zip(col, y)), data["l1"])
            / sum(v * v for v in col)
            for col in columns
        ]
        return {
            "coefficients": beta,
            "intercept": my - sum(a * b for a, b in zip(means, beta)),
        }
    if family == "slope":
        order = sorted(range(len(y)), key=lambda i: -abs(y[i]))
        shifted = [abs(y[i]) - len(y) * w for i, w in zip(order, data["weights"])]
        projected = [-v for v in pool([-v for v in shifted])]
        beta = [0.0] * len(y)
        for i, value in zip(order, projected):
            beta[i] = max(value, 0) * (1 if y[i] >= 0 else -1)
    elif family == "clustered-lasso":
        x = data["x"]
        # A Frobenius bound gives a safe step without numerical dependencies.
        lipschitz = sum(v * v for row in x for v in row)
        beta = [0.0] * len(x[0])
        for _ in range(2000):
            residual = [
                sum(a * b for a, b in zip(row, beta)) - target
                for row, target in zip(x, y)
            ]
            point = [
                b - sum(row[j] * r for row, r in zip(x, residual)) / lipschitz
                for j, b in enumerate(beta)
            ]
            order = sorted(range(len(point)), key=point.__getitem__)
            shifted = [
                point[i] + data["fusion"] / lipschitz * (len(point) - 1 - 2 * k)
                for k, i in enumerate(order)
            ]
            updated = [0.0] * len(beta)
            for i, value in zip(order, pool(shifted)):
                updated[i] = soft(value, data["l1"] / lipschitz)
            if max(abs(a - b) for a, b in zip(updated, beta)) < 1e-14:
                beta = updated
                break
            beta = updated
    else:
        raise ValueError(f"Unknown family: {family}")
    return {"coefficients": beta, "intercept": 0.0}
