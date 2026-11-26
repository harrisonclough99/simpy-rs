use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;

use fastrand::Rng;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

// ============================================================================
// Monitor: Zero-overhead Rust time-series & sample statistics recorder
// ============================================================================

struct MonitorInner {
    times: Vec<f64>,
    values: Vec<f64>,
    sum: f64,
    sum_sq: f64,
    min: f64,
    max: f64,
}

/// High-performance Rust statistics & time-series monitor for simulation metrics.
#[pyclass(module = "simpy_rs._simpy_rs")]
pub struct Monitor {
    inner: Mutex<MonitorInner>,
}

#[pymethods]
impl Monitor {
    #[new]
    fn py_new() -> Self {
        Self {
            inner: Mutex::new(MonitorInner {
                times: Vec::new(),
                values: Vec::new(),
                sum: 0.0,
                sum_sq: 0.0,
                min: f64::INFINITY,
                max: f64::NEG_INFINITY,
            }),
        }
    }

    /// Record an observation `(time, value)`.
    #[pyo3(signature = (value, time = 0.0))]
    fn record(&self, value: f64, time: f64) {
        let mut inner = self.inner.lock().unwrap();
        inner.times.push(time);
        inner.values.push(value);
        inner.sum += value;
        inner.sum_sq += value * value;
        if value < inner.min {
            inner.min = value;
        }
        if value > inner.max {
            inner.max = value;
        }
    }

    /// Record a batch of `(time, value)` observations in one FFI call.
    fn record_batch(&self, times: Vec<f64>, values: Vec<f64>) -> PyResult<()> {
        if times.len() != values.len() {
            return Err(PyValueError::new_err(
                "times and values must have the same length",
            ));
        }
        let mut inner = self.inner.lock().unwrap();
        inner.times.reserve(times.len());
        inner.values.reserve(values.len());
        for (t, v) in times.into_iter().zip(values) {
            inner.times.push(t);
            inner.values.push(v);
            inner.sum += v;
            inner.sum_sq += v * v;
            if v < inner.min {
                inner.min = v;
            }
            if v > inner.max {
                inner.max = v;
            }
        }
        Ok(())
    }

    #[getter]
    fn count(&self) -> usize {
        self.inner.lock().unwrap().values.len()
    }

    #[getter]
    fn mean(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        if inner.values.is_empty() {
            0.0
        } else {
            inner.sum / inner.values.len() as f64
        }
    }

    #[getter]
    fn variance(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        let n = inner.values.len();
        if n < 2 {
            return 0.0;
        }
        let n_f = n as f64;
        let mean = inner.sum / n_f;
        ((inner.sum_sq / n_f) - mean * mean).max(0.0)
    }

    #[getter]
    fn std(&self) -> f64 {
        self.variance().sqrt()
    }

    #[getter]
    fn min(&self) -> Option<f64> {
        let inner = self.inner.lock().unwrap();
        if inner.values.is_empty() {
            None
        } else {
            Some(inner.min)
        }
    }

    #[getter]
    fn max(&self) -> Option<f64> {
        let inner = self.inner.lock().unwrap();
        if inner.values.is_empty() {
            None
        } else {
            Some(inner.max)
        }
    }

    /// Compute a percentile `q` in `[0.0, 100.0]`.
    fn percentile(&self, q: f64) -> PyResult<f64> {
        if !(0.0..=100.0).contains(&q) || q.is_nan() {
            return Err(PyValueError::new_err("q must be between 0 and 100"));
        }
        let inner = self.inner.lock().unwrap();
        if inner.values.is_empty() {
            return Ok(0.0);
        }
        let mut sorted = inner.values.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let rank = (q / 100.0) * (sorted.len() - 1) as f64;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        let frac = rank - lo as f64;
        Ok(sorted[lo] * (1.0 - frac) + sorted[hi] * frac)
    }

    /// Compute the time-weighted average over `[t_0, until]`.
    #[pyo3(signature = (until = None))]
    fn time_average(&self, until: Option<f64>) -> f64 {
        let inner = self.inner.lock().unwrap();
        let n = inner.values.len();
        if n == 0 {
            return 0.0;
        }
        let end_time = until.unwrap_or_else(|| inner.times[n - 1]);
        let start_time = inner.times[0];
        let total_dt = end_time - start_time;
        if total_dt <= 0.0 {
            return inner.values[n - 1];
        }
        let mut area = 0.0;
        for i in 0..n - 1 {
            let dt = inner.times[i + 1] - inner.times[i];
            if dt > 0.0 {
                area += inner.values[i] * dt;
            }
        }
        let tail_dt = end_time - inner.times[n - 1];
        if tail_dt > 0.0 {
            area += inner.values[n - 1] * tail_dt;
        }
        area / total_dt
    }

    fn times(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        Ok(PyList::new(py, &inner.times)?.unbind())
    }

    fn values(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let inner = self.inner.lock().unwrap();
        Ok(PyList::new(py, &inner.values)?.unbind())
    }

    fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.times.clear();
        inner.values.clear();
        inner.sum = 0.0;
        inner.sum_sq = 0.0;
        inner.min = f64::INFINITY;
        inner.max = f64::NEG_INFINITY;
    }

    fn __len__(&self) -> usize {
        self.count()
    }
}

// ============================================================================
// Native Zero-GIL Queueing Simulation Runners (M/M/1 and M/M/c)
// ============================================================================

#[derive(Copy, Clone)]
enum QueueEventKind {
    Arrival,
    Departure,
}

#[derive(Copy, Clone)]
struct QueueEvent {
    time: f64,
    seq: u64,
    kind: QueueEventKind,
}

impl PartialEq for QueueEvent {
    fn eq(&self, other: &Self) -> bool {
        self.time.to_bits() == other.time.to_bits() && self.seq == other.seq
    }
}

impl Eq for QueueEvent {}

impl PartialOrd for QueueEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .total_cmp(&self.time)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

#[inline]
fn sample_exp(rng: &mut Rng, rate: f64) -> f64 {
    // -ln(1 - U) / rate where U in [0, 1)
    let u = rng.f64();
    -(1.0 - u).ln() / rate
}

pub struct MmcStats {
    pub arrivals: u64,
    pub completed: u64,
    pub mean_wait: f64,
    pub max_wait: f64,
    pub mean_system_time: f64,
    pub mean_queue_length: f64,
    pub server_utilization: f64,
    pub sim_time: f64,
}

pub fn run_mmc_native(
    arrival_rate: f64,
    service_rate: f64,
    servers: usize,
    until: f64,
    seed: u64,
) -> MmcStats {
    let mut rng = Rng::with_seed(seed);
    let mut heap = BinaryHeap::with_capacity(servers + 64);
    let mut wait_queue = std::collections::VecDeque::<f64>::new();

    let mut seq: u64 = 0;
    let first_arrival = sample_exp(&mut rng, arrival_rate);
    if first_arrival < until {
        heap.push(QueueEvent {
            time: first_arrival,
            seq,
            kind: QueueEventKind::Arrival,
        });
        seq += 1;
    }

    let mut now = 0.0_f64;
    let mut busy: usize = 0;
    let mut arrivals: u64 = 0;
    let mut completed: u64 = 0;
    let mut total_wait = 0.0_f64;
    let mut max_wait = 0.0_f64;
    let mut total_system = 0.0_f64;
    let mut area_queue = 0.0_f64;
    let mut area_busy = 0.0_f64;

    while let Some(ev) = heap.pop() {
        if ev.time > until {
            break;
        }
        let dt = ev.time - now;
        if dt > 0.0 {
            area_queue += wait_queue.len() as f64 * dt;
            area_busy += busy as f64 * dt;
            now = ev.time;
        }

        match ev.kind {
            QueueEventKind::Arrival => {
                arrivals += 1;
                let next_arr = now + sample_exp(&mut rng, arrival_rate);
                if next_arr <= until {
                    heap.push(QueueEvent {
                        time: next_arr,
                        seq,
                        kind: QueueEventKind::Arrival,
                    });
                    seq += 1;
                }

                if busy < servers {
                    busy += 1;
                    let svc = sample_exp(&mut rng, service_rate);
                    total_system += svc;
                    heap.push(QueueEvent {
                        time: now + svc,
                        seq,
                        kind: QueueEventKind::Departure,
                    });
                    seq += 1;
                } else {
                    wait_queue.push_back(now);
                }
            }
            QueueEventKind::Departure => {
                completed += 1;
                if let Some(arr_time) = wait_queue.pop_front() {
                    let wait = now - arr_time;
                    total_wait += wait;
                    if wait > max_wait {
                        max_wait = wait;
                    }
                    let svc = sample_exp(&mut rng, service_rate);
                    total_system += wait + svc;
                    heap.push(QueueEvent {
                        time: now + svc,
                        seq,
                        kind: QueueEventKind::Departure,
                    });
                    seq += 1;
                } else {
                    busy -= 1;
                }
            }
        }
    }

    if until > now {
        let dt = until - now;
        area_queue += wait_queue.len() as f64 * dt;
        area_busy += busy as f64 * dt;
        now = until;
    }

    let served_or_started = completed + busy as u64;
    MmcStats {
        arrivals,
        completed,
        mean_wait: if served_or_started > 0 {
            total_wait / served_or_started as f64
        } else {
            0.0
        },
        max_wait,
        mean_system_time: if served_or_started > 0 {
            total_system / served_or_started as f64
        } else {
            0.0
        },
        mean_queue_length: if until > 0.0 { area_queue / until } else { 0.0 },
        server_utilization: if until > 0.0 && servers > 0 {
            area_busy / (until * servers as f64)
        } else {
            0.0
        },
        sim_time: now,
    }
}

/// Run a full M/M/c discrete-event queueing simulation natively in Rust (zero GIL/generator overhead).
#[pyfunction]
#[pyo3(signature = (arrival_rate, service_rate, servers = 1, until = 10_000.0, seed = 42))]
pub fn simulate_mmc<'py>(
    py: Python<'py>,
    arrival_rate: f64,
    service_rate: f64,
    servers: usize,
    until: f64,
    seed: u64,
) -> PyResult<Bound<'py, PyDict>> {
    if arrival_rate <= 0.0 || arrival_rate.is_nan() {
        return Err(PyValueError::new_err("arrival_rate must be > 0"));
    }
    if service_rate <= 0.0 || service_rate.is_nan() {
        return Err(PyValueError::new_err("service_rate must be > 0"));
    }
    if servers == 0 {
        return Err(PyValueError::new_err("servers must be >= 1"));
    }
    if until <= 0.0 || until.is_nan() {
        return Err(PyValueError::new_err("until must be > 0"));
    }

    let stats = py.detach(|| run_mmc_native(arrival_rate, service_rate, servers, until, seed));

    let dict = PyDict::new(py);
    dict.set_item("arrivals", stats.arrivals)?;
    dict.set_item("completed", stats.completed)?;
    dict.set_item("mean_wait", stats.mean_wait)?;
    dict.set_item("max_wait", stats.max_wait)?;
    dict.set_item("mean_system_time", stats.mean_system_time)?;
    dict.set_item("mean_queue_length", stats.mean_queue_length)?;
    dict.set_item("server_utilization", stats.server_utilization)?;
    dict.set_item("sim_time", stats.sim_time)?;
    Ok(dict)
}

/// Run a full M/M/1 discrete-event queueing simulation natively in Rust.
#[pyfunction]
#[pyo3(signature = (arrival_rate, service_rate, until = 10_000.0, seed = 42))]
pub fn simulate_mm1<'py>(
    py: Python<'py>,
    arrival_rate: f64,
    service_rate: f64,
    until: f64,
    seed: u64,
) -> PyResult<Bound<'py, PyDict>> {
    simulate_mmc(py, arrival_rate, service_rate, 1, until, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mm1_matches_queuing_theory() {
        // M/M/1 with lambda=0.8, mu=1.0 -> rho=0.8, W_q = rho / (mu - lambda) = 4.0, L_q = 3.2
        let stats = run_mmc_native(0.8, 1.0, 1, 200_000.0, 42);
        assert!(stats.completed > 150_000);
        assert!((stats.server_utilization - 0.8).abs() < 0.03);
        assert!((stats.mean_wait - 4.0).abs() < 0.4);
        assert!((stats.mean_queue_length - 3.2).abs() < 0.4);
    }

    #[test]
    fn mmc_is_deterministic() {
        let a = run_mmc_native(2.5, 1.0, 3, 10_000.0, 12345);
        let b = run_mmc_native(2.5, 1.0, 3, 10_000.0, 12345);
        assert_eq!(a.arrivals, b.arrivals);
        assert_eq!(a.completed, b.completed);
        assert_eq!(a.mean_wait.to_bits(), b.mean_wait.to_bits());
    }
}
