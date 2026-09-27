// Vendored from lsap 1.0.2 (MIT). See ../NOTICE for the license notice.

/// Solve the linear sum assignment problem and return a tuple of vectors containing the assigned
///
/// The implementation is translated from the C++ code from [Scipy](https://docs.scipy.org/doc/scipy/reference/generated/scipy.optimize.linear_sum_assignment.html).
///
/// # Arguments
///
/// * `nr` - number of rows in the cost matrix
/// * `nc` - number of columns in the cost matrix
/// * `cost` - cost matrix flattened into a vector such that item at row i, column j can be accessed via cost[i * nc + j]
/// * `maximize` - if true, solve the maximization problem instead of the minimization problem
pub(crate) fn solve(
    mut nr: usize,
    mut nc: usize,
    cost: &Vec<f64>,
    maximize: bool,
) -> Result<(Vec<usize>, Vec<usize>), &'static str> {
    // handle trivial inputs
    if nr == 0 || nc == 0 {
        return Ok((vec![], vec![]));
    }

    // tall rectangular cost matrix must be transposed
    let transpose = nc < nr;

    // make a copy of the cost matrix if we need to modify it
    let mut temp: Vec<f64>;
    let surrogated_cost = if transpose || maximize {
        if transpose {
            temp = vec![0.0; nc * nr];
            for i in 0..nr {
                for j in 0..nc {
                    temp[j * nr + i] = cost[i * nc + j];
                }
            }

            std::mem::swap(&mut nr, &mut nc);
        } else {
            temp = cost.clone();
        }

        // negate cost matrix for maximization
        if maximize {
            for i in 0..(nr * nc) {
                temp[i] = -temp[i];
            }
        }

        &temp
    } else {
        cost
    };

    // test for NaN and -inf entries
    for i in 0..(nr * nc) {
        if surrogated_cost[i].is_nan() || surrogated_cost[i].is_infinite() {
            return Err("invalid assignment cost");
        }
    }

    // initialize variables
    let MINUS_1: usize = nr * nc; // use this to represent -1 in the C++ code, it has the same effect

    let mut u = vec![0.0; nr];
    let mut v = vec![0.0; nc];
    let mut shortest_path_costs: Vec<f64> = vec![f64::INFINITY; nc];
    let mut path: Vec<usize> = vec![MINUS_1; nc];
    let mut col4row: Vec<usize> = vec![MINUS_1; nr];
    let mut row4col: Vec<usize> = vec![MINUS_1; nc];
    let mut SR: Vec<bool> = vec![false; nr];
    let mut SC: Vec<bool> = vec![false; nc];
    let mut remaining: Vec<usize> = vec![MINUS_1; nc];

    // iteratively build the solution
    for cur_row in 0..nr {
        let (sink, min_val) = augmenting_path(
            nc,
            &surrogated_cost,
            &mut u,
            &mut v,
            &mut path,
            &row4col,
            &mut shortest_path_costs,
            cur_row,
            &mut SR,
            &mut SC,
            &mut remaining,
            MINUS_1,
        );

        if sink == MINUS_1 {
            return Err("infeasible assignment");
        }

        // update dual variables
        u[cur_row] += min_val;
        for i in 0..nr {
            if SR[i] && i != cur_row {
                u[i] += min_val - shortest_path_costs[col4row[i]];
            }
        }

        for j in 0..nc {
            if SC[j] {
                v[j] -= min_val - shortest_path_costs[j];
            }
        }

        // augment previous solution
        let mut j = sink;
        loop {
            let i = path[j];
            row4col[j] = i;
            std::mem::swap(&mut col4row[i], &mut j);
            if i == cur_row {
                break;
            }
        }
    }

    let mut a = Vec::with_capacity(nr);
    let mut b = Vec::with_capacity(nr);

    if transpose {
        for v in argsort_iter(&col4row) {
            a.push(col4row[v]);
            b.push(v);
        }
    } else {
        for i in 0..nr {
            a.push(i);
            b.push(col4row[i]);
        }
    }

    return Ok((a, b));
}

fn augmenting_path(
    nc: usize,
    cost: &Vec<f64>,
    u: &mut Vec<f64>,
    v: &mut Vec<f64>,
    path: &mut Vec<usize>,
    row4col: &Vec<usize>,
    shortest_path_costs: &mut Vec<f64>,
    mut i: usize,
    SR: &mut Vec<bool>,
    SC: &mut Vec<bool>,
    remaining: &mut Vec<usize>,
    MINUS_1: usize,
) -> (usize, f64) {
    let mut min_val = 0.0;

    // Crouse's pseudocode uses set complements to keep track of remaining
    // nodes.  Here we use a vector, as it is more efficient in C++ (Rust?).
    let mut num_remaining = nc;
    for it in 0..nc {
        // Filling this up in reverse order ensures that the solution of a
        // constant cost matrix is the identity matrix (c.f. #11602).
        remaining[it] = nc - it - 1;
    }

    SR.fill(false);
    SC.fill(false);
    shortest_path_costs.fill(f64::INFINITY);

    // find shortest augmenting path
    let mut sink = MINUS_1;
    while sink == MINUS_1 {
        let mut index = MINUS_1;
        let mut lowest = f64::INFINITY;
        SR[i] = true;

        for it in 0..num_remaining {
            let j = remaining[it];

            let r: f64 = min_val + cost[i * nc + j] - u[i] - v[j];
            if r < shortest_path_costs[j] {
                path[j] = i;
                shortest_path_costs[j] = r;
            }

            // When multiple nodes have the minimum cost, we select one which
            // gives us a new sink node. This is particularly important for
            // integer cost matrices with small co-efficients.
            if shortest_path_costs[j] < lowest
                || (shortest_path_costs[j] == lowest && row4col[j] == MINUS_1)
            {
                lowest = shortest_path_costs[j];
                index = it;
            }
        }

        min_val = lowest;
        if min_val.is_infinite() {
            // infeasible cost matrix
            return (MINUS_1, min_val); // returns min_val but it won't be used
        }

        let j = remaining[index];
        if row4col[j] == MINUS_1 {
            sink = j;
        } else {
            i = row4col[j];
        }

        SC[j] = true;
        num_remaining -= 1;
        remaining[index] = remaining[num_remaining];
    }

    return (sink, min_val); // they assign p_minVal, we return instead
}

fn argsort_iter<T: Ord>(v: &Vec<T>) -> Vec<usize> {
    let mut index = (0..v.len()).collect::<Vec<_>>();
    index.sort_by_key(|&i| &v[i]);
    index
}
