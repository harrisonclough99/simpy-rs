use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

create_exception!(
    simpy_rs._simpy_rs,
    SimPyException,
    PyException,
    "Base class for all SimPy-specific exceptions."
);

create_exception!(
    simpy_rs._simpy_rs,
    Interrupt,
    SimPyException,
    "Exception thrown into a process if it is interrupted."
);

create_exception!(
    simpy_rs._simpy_rs,
    EmptySchedule,
    PyException,
    "Thrown by an Environment if there are no further events to be processed."
);

create_exception!(
    simpy_rs._simpy_rs,
    StopSimulation,
    PyException,
    "Indicates that the simulation should stop now."
);

#[pyfunction]
fn _interrupt_get_cause(slf: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    let py = slf.py();
    let args = slf.getattr("args")?;
    let tuple = args.cast::<PyTuple>()?;
    if tuple.is_empty() {
        Ok(py.None())
    } else {
        Ok(tuple.get_item(0)?.unbind())
    }
}

#[pyfunction]
fn _interrupt_str(slf: &Bound<'_, PyAny>) -> PyResult<String> {
    let cls_name = slf.get_type().name()?;
    let cause = _interrupt_get_cause(slf)?;
    let cause_repr = cause.bind(slf.py()).repr()?;
    Ok(format!("{cls_name}({cause_repr})"))
}

#[pyfunction]
fn _stop_simulation_get_value(slf: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    let py = slf.py();
    let args = slf.getattr("args")?;
    let tuple = args.cast::<PyTuple>()?;
    if tuple.is_empty() {
        Ok(py.None())
    } else {
        Ok(tuple.get_item(0)?.unbind())
    }
}

#[pyfunction]
fn _stop_simulation_callback(cls: &Bound<'_, PyAny>, event: &Bound<'_, PyAny>) -> PyResult<()> {
    let ok: bool = event.getattr("ok")?.extract()?;
    let value = event.getattr("value")?;
    if ok {
        let exc = cls.call1((value,))?;
        Err(PyErr::from_value(exc))
    } else {
        Err(PyErr::from_value(value))
    }
}

pub fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let builtins = py.import("builtins")?;
    let property = builtins.getattr("property")?;
    let classmethod = builtins.getattr("classmethod")?;

    let interrupt_ty = py.get_type::<Interrupt>();
    let cause_getter = wrap_pyfunction!(_interrupt_get_cause, py)?;
    let interrupt_str = wrap_pyfunction!(_interrupt_str, py)?;
    interrupt_ty.setattr("cause", property.call1((cause_getter,))?)?;
    interrupt_ty.setattr("__str__", interrupt_str)?;

    let stop_ty = py.get_type::<StopSimulation>();
    let value_getter = wrap_pyfunction!(_stop_simulation_get_value, py)?;
    let cb_fn = wrap_pyfunction!(_stop_simulation_callback, py)?;
    stop_ty.setattr("value", property.call1((value_getter,))?)?;
    stop_ty.setattr("callback", classmethod.call1((cb_fn,))?)?;

    m.add("SimPyException", py.get_type::<SimPyException>())?;
    m.add("Interrupt", interrupt_ty)?;
    m.add("EmptySchedule", py.get_type::<EmptySchedule>())?;
    m.add("StopSimulation", stop_ty)?;
    Ok(())
}
