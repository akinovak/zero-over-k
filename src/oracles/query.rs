use ark_ff::{Field, PrimeField};

use super::rotation::Rotation;

pub type DomainSize = usize;
pub type OmegaI = usize;

pub enum QueryContext<F: Field> {
    Challenge(F),
    ExtendedCoset(DomainSize, Rotation, OmegaI),
}

impl<F: PrimeField> QueryContext<F> {
    pub fn replace_omega(&mut self, new_row: usize) {
        match self {
            Self::ExtendedCoset(_, _, old_w) => {
                let _ = std::mem::replace(old_w, new_row);
            }
            Self::Challenge(_) => {
                panic!("Can't replace omega in Challenge")
            }
        }
    }

    pub fn replace_rotation(&mut self, new_rot: Rotation) {
        match self {
            Self::ExtendedCoset(_, old_rotation, _) => {
                let _ = std::mem::replace(old_rotation, new_rot);
            }
            Self::Challenge(_) => {
                panic!("Can't replace rotation in Challenge")
            }
        }
    }
}

#[derive(Clone)]
pub enum OracleType {
    Witness,
    Instance,
    Fixed,
    // TODO: add Selector
}

#[derive(Clone)]
pub struct OracleQuery {
    pub label: String, //TODO: maybe consider: pub oracle: Box<&'a dyn ConcreteOracle<F>>,
    pub rotation: Rotation,
    pub oracle_type: OracleType,
}