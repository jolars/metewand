def prepare_path(data, context):
    parameters = context.problem_parameters
    # A shortened path still defines the same comparison for every worker.
    length = min(parameters["path_length"], len(data["values"]) - 2)
    scale = max(data["values"]) + int(parameters["fit_intercept"])
    return {"alpha": [scale * parameters["q"] ** index for index in range(length)]}
