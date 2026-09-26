//! Python objects in, [`Value`]s out, and back.

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PySequence, PyString, PyTuple};

use stanchion_ffi::Value;

/// How deep a Python structure may nest before conversion gives up.
///
/// Matches the limit on the Lua side, and for the same reason: a list that contains
/// itself is easy to build and would otherwise overflow the stack.
const MAX_DEPTH: usize = 64;

/// Converts a Python object into the value a plugin will see.
pub fn from_py(object: &Bound<'_, PyAny>) -> PyResult<Value> {
    from_py_at(object, 0)
}

fn from_py_at(object: &Bound<'_, PyAny>, depth: usize) -> PyResult<Value> {
    if depth > MAX_DEPTH {
        return Err(PyTypeError::new_err(format!(
            "value nests deeper than {MAX_DEPTH} levels, or contains itself"
        )));
    }
    let next = depth.saturating_add(1);

    if object.is_none() {
        return Ok(Value::Nil);
    }
    // Before `int`: in Python `bool` is a subclass of `int`, so asking the other way
    // round would turn every `True` into `1`.
    if let Ok(value) = object.cast::<PyBool>() {
        return Ok(Value::Bool(value.is_true()));
    }
    if let Ok(value) = object.cast::<PyInt>() {
        return Ok(Value::Int(value.extract()?));
    }
    if let Ok(value) = object.cast::<PyFloat>() {
        return Ok(Value::Float(value.extract()?));
    }
    if let Ok(value) = object.cast::<PyString>() {
        return Ok(Value::Str(value.extract()?));
    }
    if let Ok(mapping) = object.cast::<PyDict>() {
        let mut entries = std::collections::BTreeMap::new();
        for (key, value) in mapping.iter() {
            // Lua table keys arrive as strings, so this is the same shape in reverse.
            let key = match key.cast::<PyString>() {
                Ok(key) => key.extract::<String>()?,
                Err(_) => key.str()?.extract::<String>()?,
            };
            entries.insert(key, from_py_at(&value, next)?);
        }
        return Ok(Value::Map(entries));
    }
    // `str` and `bytes` are sequences too, so this has to come after them.
    if object.cast::<PyList>().is_ok() || object.cast::<PyTuple>().is_ok() {
        let sequence = object.cast::<PySequence>()?;
        let length = sequence.len()?;
        let mut items = Vec::with_capacity(length);
        for index in 0..length {
            items.push(from_py_at(&sequence.get_item(index)?, next)?);
        }
        return Ok(Value::List(items));
    }

    Err(PyTypeError::new_err(format!(
        "cannot pass a {} to a Lua plugin; use None, bool, int, float, str, list, \
         tuple or dict",
        object.get_type().name()?,
    )))
}

/// Converts a plugin's value into the Python object a caller gets back.
pub fn to_py<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::Nil => py.None().into_bound(py),
        Value::Bool(value) => value.into_pyobject(py)?.to_owned().into_any(),
        Value::Int(value) => value.into_pyobject(py)?.into_any(),
        Value::Float(value) => value.into_pyobject(py)?.into_any(),
        Value::Str(value) => value.into_pyobject(py)?.into_any(),
        Value::List(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(to_py(py, item)?)?;
            }
            list.into_any()
        }
        Value::Map(entries) => {
            let dict = PyDict::new(py);
            for (key, entry) in entries {
                dict.set_item(key, to_py(py, entry)?)?;
            }
            dict.into_any()
        }
        Value::Function => py.None().into_bound(py),
    })
}
