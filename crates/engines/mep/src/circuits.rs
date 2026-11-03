//! Electrical circuits (spec/schema.md `Circuit`): a panel feeding loads through a conductor.
//!
//! * current `I = Σ load_watts / voltage`,
//! * voltage drop `ΔV = I · 2 · ρ · L / A` (out and back through the conductor of resistivity `ρ`,
//!   length `L` and cross-section `A`),
//! * `voltage_drop_pct = 100 · ΔV / voltage`.
//!
//! The formulas are evaluated in exactly this order (left to right), so the results are the
//! correctly rounded closed forms to a few ulps.

/// A solved circuit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircuitResult {
    /// Total current drawn by the loads (A).
    pub current: f64,
    /// Voltage lost in the conductor (V).
    pub voltage_drop: f64,
    /// `voltage_drop` as a percentage of the supply voltage.
    pub voltage_drop_pct: f64,
}

/// The circuit feeding loads of `watts` (W, summed in order) at `voltage` (V) through a conductor
/// of cross-section `conductor_area` (m²), resistivity `resistivity` (Ω·m) and length `length` (m).
/// `voltage` and `conductor_area` must be non-zero for a finite result.
pub fn circuit(watts: &[f64], voltage: f64, conductor_area: f64, resistivity: f64, length: f64) -> CircuitResult {
    let current = watts.iter().sum::<f64>() / voltage;
    let voltage_drop = current * 2.0 * resistivity * length / conductor_area;
    CircuitResult { current, voltage_drop, voltage_drop_pct: 100.0 * voltage_drop / voltage }
}
