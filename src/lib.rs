#![allow(dead_code)]
#[macro_use]
#[cfg(feature = "interactive")]
extern crate pest_derive;
use anyhow::*;
use compiler::ConstraintSet;
use errno::{set_errno, Errno};
use libc::c_char;
use log::*;
use pairing_ce::{
    bn256::Fr,
    ff::{Field, PrimeField},
};
use std::ffi::{c_uint, CStr, CString};

use crate::{column::Computation, compiler::EvalSettings, structs::Handle};

mod column;
mod compiler;
mod compute;
mod dag;
mod errors;
mod pretty;
mod structs;
mod transformer;

type Corset = ConstraintSet;

pub const ERR_NOT_AN_USIZE: i32 = 1;
pub const ERR_COMPUTE_TRACE_FAILED: i32 = 2;
pub const ERR_COLUMN_NAME_NOT_FOUND: i32 = 3;
pub const ERR_COULD_NOT_INITIALIZE_RAYON: i32 = 4;
pub const ERR_COLUMN_ID_NOT_FOUND: i32 = 5;
pub const ERR_INVALID_ZKEVM_FILE: i32 = 6;

fn cstr_to_string(s: *const c_char) -> String {
    let name = unsafe {
        assert!(!s.is_null());
        CStr::from_ptr(s)
    };

    name.to_str().unwrap().to_owned()
}

struct ComputedColumn {
    padding_value: [u64; 4],
    values: Vec<[u64; 4]>,
}
#[derive(Default)]
pub struct Trace {
    columns: Vec<ComputedColumn>,
    ids: Vec<String>,
}
impl Trace {
    fn from_constraints(c: &Corset) -> Self {
        let mut r = Trace {
            ..Default::default()
        };

        for (module, columns) in c.modules.cols.iter() {
            let empty_vec = Vec::new();
            for (name, &i) in columns.iter() {
                let column = &c.modules._cols[i];
                let handle = Handle::new(&module, &name);
                let value = column.value().unwrap_or(&empty_vec);
                let padding = if let Some(x) = column.padding_value {
                    Fr::from_str(&x.to_string()).unwrap()
                } else {
                    value.get(0).cloned().unwrap_or_else(|| {
                        c.computations
                            .computation_for(&handle)
                            .map(|c| match c {
                                Computation::Composite { exp, .. } => exp
                                    .eval(
                                        0,
                                        &mut |_, _, _| Some(Fr::zero()),
                                        &mut None,
                                        &EvalSettings::default(),
                                    )
                                    .unwrap_or_else(Fr::zero),
                                Computation::Interleaved { .. } => Fr::zero(),
                                Computation::Sorted { .. } => Fr::zero(),
                                Computation::CyclicFrom { .. } => Fr::zero(),
                                Computation::SortingConstraints { .. } => Fr::zero(),
                            })
                            .unwrap_or_else(Fr::zero)
                    })
                };
                r.columns.push(ComputedColumn {
                    values: column
                        .value()
                        .unwrap_or(&empty_vec)
                        .iter()
                        .map(|x| x.into_repr().0)
                        .collect(),
                    padding_value: padding.into_repr().0,
                });
                r.ids.push(handle.mangle());
            }
        }
        r
    }
    fn from_ptr<'a>(ptr: *const Trace) -> &'a Self {
        assert!(!ptr.is_null());
        unsafe { &*ptr }
    }
}

fn _load_corset(zkevmfile: &str) -> Result<Corset> {
    info!("Loading `{}`", &zkevmfile);
    let mut constraints = ron::from_str(
        &std::fs::read_to_string(&zkevmfile)
            .with_context(|| anyhow!("while reading `{}`", zkevmfile))?,
    )
    .with_context(|| anyhow!("while parsing `{}`", zkevmfile))?;

    transformer::validate_nhood(&mut constraints)?;
    transformer::lower_shifts(&mut constraints)?;
    transformer::expand_ifs(&mut constraints);
    transformer::expand_constraints(&mut constraints)?;
    transformer::sorts(&mut constraints)?;
    transformer::expand_invs(&mut constraints)?;

    Ok(constraints)
}

fn _compute_trace(
    constraints: &mut Corset,
    tracefile: &str,
    fail_on_missing: bool,
) -> Result<Trace> {
    compute::compute_trace(
        &compute::read_trace(&tracefile)?,
        constraints,
        fail_on_missing,
    )
    .with_context(|| format!("while computing from `{}`", tracefile))?;
    Ok(Trace::from_constraints(constraints))
}

#[no_mangle]
pub extern "C" fn load_corset(zkevmfile: *const c_char) -> *mut Corset {
    let zkevmfile = cstr_to_string(zkevmfile);
    match _load_corset(&zkevmfile) {
        Result::Ok(constraints) => Box::into_raw(Box::new(constraints)),
        Err(e) => {
            eprintln!("{:?}", e);
            set_errno(Errno(ERR_INVALID_ZKEVM_FILE));
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn trace_check(
    corset: *mut Corset,
    tracefile: *const c_char,
    threads: c_uint,
    fail_on_missing: bool,
) -> bool {
    if rayon::ThreadPoolBuilder::new()
        .num_threads(if let Result::Ok(t) = threads.try_into() {
            t
        } else {
            set_errno(Errno(ERR_NOT_AN_USIZE));
            return false;
        })
        .build_global()
        .is_err()
    {
        set_errno(Errno(ERR_COULD_NOT_INITIALIZE_RAYON));
        return false;
    }

    let _tracefile = cstr_to_string(tracefile);
    let _constraints = Corset::mut_from_ptr(corset);

    todo!()
}

#[no_mangle]
pub extern "C" fn trace_compute(
    corset: *mut Corset,
    tracefile: *const c_char,
    threads: c_uint,
    fail_on_missing: bool,
) -> *mut Trace {
    if rayon::ThreadPoolBuilder::new()
        .num_threads(if let Result::Ok(t) = threads.try_into() {
            t
        } else {
            set_errno(Errno(ERR_NOT_AN_USIZE));
            return std::ptr::null_mut();
        })
        .build_global()
        .is_err()
    {
        set_errno(Errno(ERR_COULD_NOT_INITIALIZE_RAYON));
        return std::ptr::null_mut();
    }

    let tracefile = cstr_to_string(tracefile);
    let constraints = Corset::mut_from_ptr(corset);
    let r = _compute_trace(constraints, &tracefile, fail_on_missing);
    match r {
        Err(e) => {
            eprintln!("{:?}", e);
            set_errno(Errno(ERR_COMPUTE_TRACE_FAILED));
            std::ptr::null_mut()
        }
        Result::Ok(x) => Box::into_raw(Box::new(x)),
    }
}

#[no_mangle]
pub extern "C" fn trace_free(trace: *mut Trace) {
    if !trace.is_null() {
        unsafe {
            drop(Box::from_raw(trace));
        }
    }
}

#[no_mangle]
pub extern "C" fn trace_column_count(trace: *const Trace) -> c_uint {
    let r = Trace::from_ptr(trace);
    r.ids.len() as c_uint
}

#[no_mangle]
pub extern "C" fn trace_column_names(trace: *const Trace) -> *const *mut c_char {
    let r = Trace::from_ptr(trace);
    let names = r
        .ids
        .iter()
        .map(|s| CString::new(s.as_str()).unwrap().into_raw())
        .collect::<Vec<_>>();
    let ptr = names.as_ptr();

    std::mem::forget(names); // so that it is not destructed at the end of the scope

    ptr
}

#[repr(C)]
pub struct ColumnData {
    padding_value: [u64; 4],
    values: *const [u64; 4],
    values_len: u64,
}
impl Default for ColumnData {
    fn default() -> Self {
        ColumnData {
            padding_value: Default::default(),
            values: std::ptr::null(),
            values_len: 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn trace_column_by_name(trace: *const Trace, name: *const c_char) -> ColumnData {
    let r = Trace::from_ptr(trace);
    let name = cstr_to_string(name);

    let i = r.ids.iter().position(|n| *n == name);
    if let Some(i) = i {
        trace_column_by_id(trace, i.try_into().unwrap())
    } else {
        let r = Default::default();
        set_errno(Errno(ERR_COLUMN_NAME_NOT_FOUND));
        r
    }
}

#[no_mangle]
pub extern "C" fn trace_column_by_id(trace: *const Trace, i: u32) -> ColumnData {
    let r = Trace::from_ptr(trace);
    let i = i as usize;
    assert!(i < r.columns.len());
    let col = if let Some(c) = r.columns.get(i) {
        c
    } else {
        let r = ColumnData::default();
        set_errno(Errno(ERR_COLUMN_ID_NOT_FOUND));
        return r;
    };

    ColumnData {
        padding_value: col.padding_value,
        values: col.values.as_ptr(),
        values_len: col.values.len() as u64,
    }
}