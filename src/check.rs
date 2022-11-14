use cached::SizedCache;
use colored::Colorize;
#[cfg(feature = "interactive")]
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use rayon::prelude::*;
use std::collections::HashSet;

use anyhow::*;
use log::*;
use pairing_ce::{
    bn256::Fr,
    ff::{Field, PrimeField},
};

use crate::{
    column::ColumnSet,
    compiler::{Constraint, ConstraintSet, EvalSettings, Expression, Handle},
    pretty::*,
};

fn fail(expr: &Expression, i: isize, l: Option<usize>, columns: &ColumnSet<Fr>) -> Result<()> {
    let trace_span: isize = crate::SETTINGS.get().unwrap().trace_span;

    let module = expr.dependencies().iter().next().unwrap().module.clone();
    let handles = if crate::SETTINGS.get().unwrap().full_trace {
        columns
            .cols
            .get(&module)
            .unwrap()
            .keys()
            .map(|name| Handle::new(&module, &name))
            .sorted_by_key(|h| h.name.clone())
            .collect::<Vec<_>>()
    } else {
        expr.dependencies().iter().cloned().collect::<Vec<_>>()
    };

    let mut m_columns = vec![vec![String::new()]
        .into_iter()
        .chain(handles.iter().map(|h| h.to_string()))
        .collect::<Vec<_>>()];
    for j in (i - trace_span).max(0)..=i + trace_span {
        m_columns.push(
            vec![j.to_string()]
                .into_iter()
                .chain(handles.iter().map(|handle| {
                    columns
                        .get(handle)
                        .unwrap()
                        .get(j, false)
                        .map(|x| x.pretty())
                        .unwrap_or_else(|| "nil".into())
                }))
                .collect(),
        )
    }

    for ii in 0..m_columns[0].len() {
        for (j, col) in m_columns.iter().enumerate() {
            let padding = col.iter().map(String::len).max().unwrap() + 2;
            // - 1 to account for the first column
            if j as isize + (i - trace_span).max(0) - 1 == i {
                print!("{:width$}", m_columns[j][ii].red(), width = padding);
            } else {
                print!("{:width$}", m_columns[j][ii], width = padding);
            }
        }
        println!();
    }

    let r = expr.eval(
        i,
        &mut |handle, i, wrap| {
            columns
                .get(handle)
                .ok()
                .and_then(|c| c.get(i, wrap))
                .cloned()
        },
        &mut None,
        &EvalSettings::new().set_trace(true),
    );

    Err(anyhow!(
        "{}|{}{}\n -> {}",
        expr.pretty(),
        i,
        l.map(|l| format!("/{}", l)).unwrap_or_default(),
        r.as_ref()
            .map(Pretty::pretty)
            .unwrap_or_else(|| "nil".to_owned()),
    ))
}

fn check_constraint_at(
    expr: &Expression,
    i: isize,
    l: Option<usize>,
    columns: &ColumnSet<Fr>,
    fail_on_oob: bool,
    cache: &mut Option<SizedCache<Fr, Fr>>,
) -> Result<()> {
    let r = expr.eval(
        i,
        &mut |handle, i, wrap| columns._cols[handle.id.unwrap()].get(i, wrap).cloned(),
        cache,
        &Default::default(),
    );
    if let Some(r) = r {
        if !r.is_zero() {
            return fail(expr, i, l, columns);
        }
    } else if fail_on_oob {
        return fail(expr, i, l, columns);
    }
    Ok(())
}

fn check_constraint(
    expr: &Expression,
    domain: &Option<Vec<isize>>,
    columns: &ColumnSet<Fr>,
    name: &str,
) -> Result<()> {
    let cols_lens = expr
        .dependencies()
        .into_iter()
        .map(|handle| {
            columns
                .get(&handle)
                .with_context(|| anyhow!("can not find column `{}`", handle))
                .map(|c| c.len())
        })
        .collect::<Result<Vec<_>>>()?;
    // Early exit if all the columns are empty: the module is not triggered
    // Ideally, this should be an `all` rather than an `any`, but the IC
    // pushes columns that will always be filled.
    if cols_lens.iter().any(|l| l.is_none()) {
        debug!("Skipping constraint `{}` with empty columns", name);
        return Ok(());
    }
    if !cols_lens
        .iter()
        .all(|&l| l.unwrap_or_default() == cols_lens[0].unwrap_or_default())
    {
        error!(
            "all columns are not of the same length:\n{}",
            expr.dependencies()
                .iter()
                .map(|handle| format!(
                    "\t{}: {}",
                    handle,
                    columns
                        .get(handle)
                        .unwrap()
                        .len()
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "nil".into())
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let l = cols_lens[0].unwrap_or(0);
    if l == 0 {
        return Err(anyhow!("empty trace, aborting"));
    }

    let mut cache = Some(cached::SizedCache::with_size(200000)); // ~1.60MB cache
    match domain {
        Some(is) => {
            for i in is {
                check_constraint_at(expr, *i, None, columns, true, &mut cache)?;
            }
        }
        None => {
            for i in 0..l as isize {
                check_constraint_at(expr, i, Some(l), columns, false, &mut cache)?;
            }
        }
    };
    Ok(())
}

fn check_plookup(
    cs: &ConstraintSet,
    parents: &[Expression],
    children: &[Expression],
) -> Result<()> {
    fn pseudo_rlc(cols: &[Vec<Fr>], i: usize) -> Fr {
        let mut ax = Fr::zero();
        for (j, col) in cols.iter().enumerate() {
            let mut x = Fr::from_str(&(j + 2).to_string()).unwrap();
            x.mul_assign(&col[i]);
            ax.add_assign(&x);
        }
        ax
    }

    fn compute_cols(exps: &[Expression], cs: &ConstraintSet) -> Result<Vec<Vec<Fr>>> {
        let cols = exps
            .iter()
            .map(|p| cs.compute_composite_static(p))
            .collect::<Result<Vec<_>>>()
            .with_context(|| anyhow!("while computing {:?}", exps))?;
        if !cols.iter().all(|p| p.len() == cols[0].len()) {
            return Err(anyhow!("all columns should be of the same length"));
        }

        Ok(cols)
    }

    if children.len() != parents.len() {
        return Err(anyhow!("parents and children are not of the same length"));
    }
    let parent_cols = compute_cols(parents, cs)?;
    let child_cols = compute_cols(children, cs)?;

    let hashes: HashSet<_> = (0..parent_cols[0].len())
        .map(|i| pseudo_rlc(&parent_cols, i))
        .collect();

    for i in 0..child_cols[0].len() {
        let ax = pseudo_rlc(&child_cols, i);

        if !hashes.contains(&ax) {
            return Err(anyhow!(
                "{{\n{}\n}} not found in {{{}}}",
                children
                    .iter()
                    .zip(child_cols.iter().map(|c| c[i]))
                    .map(|(k, v)| format!("{}: {}", k, v.pretty()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                parents
                    .iter()
                    .map(|k| format!("{}", k))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    Ok(())
}

pub fn check(
    cs: &ConstraintSet,
    only: &Option<Vec<String>>,
    skip: &[String],
    with_bar: bool,
) -> Result<()> {
    if cs.modules.is_empty() {
        return Ok(());
    }

    #[cfg(feature = "interactive")]
    let bar = if with_bar {
        {
            Some(
                ProgressBar::new(cs.constraints.len() as u64).with_style(
                    ProgressStyle::default_bar()
                        .template("Validating {msg} {bar:40} {pos}/{len}")
                        .unwrap()
                        .progress_chars("##-"),
                ),
            )
        }
    } else {
        None
    };
    let failed = cs
        .constraints
        .par_iter()
        .with_max_len(1)
        .filter(|c| {
            only.as_ref()
                .map(|o| o.contains(&c.name().to_string()))
                .unwrap_or(true)
        })
        .filter(|c| !skip.contains(&c.name().to_string()))
        .inspect(|_| {
            #[cfg(feature = "interactive")]
            {
                if let Some(b) = &bar {
                    b.inc(1)
                }
            }
        })
        .filter_map(|c| {
            match c {
                Constraint::Vanishes { name, domain, expr } => {
                    if name == "INV_CONSTRAINTS" || matches!(**expr, Expression::Void) {
                        return None;
                    }

                    match expr.as_ref() {
                        Expression::List(es) => {
                            for e in es {
                                if let Err(err) = check_constraint(e, domain, &cs.modules, name) {
                                    error!("{:?}", err);
                                    return Some(name.to_owned());
                                }
                            }
                            None
                        }
                        _ => {
                            if let Err(err) = check_constraint(expr, domain, &cs.modules, name) {
                                error!("{:?}", err);
                                Some(name.to_owned())
                            } else {
                                None
                            }
                        }
                    }
                }
                Constraint::Plookup(name, parents, children) => {
                    if let Err(err) = check_plookup(cs, parents, children) {
                        error!("{:?}", err);
                        Some(name.to_owned())
                    } else {
                        None
                    }
                }
                Constraint::Permutation(_name, _from, _to) => {
                    // warn!("Permutation validation not yet implemented");
                    None
                }
                Constraint::InRange(_, _e, _range) => {
                    // warn!("Range validation not yet implemented")
                    None
                }
            }
        })
        .collect::<HashSet<_>>();
    if failed.is_empty() {
        info!("Validation successful");
        Ok(())
    } else {
        Err(anyhow!(
            "Constraints failed: {}",
            failed
                .into_iter()
                .map(|x| x.bold().red().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}
