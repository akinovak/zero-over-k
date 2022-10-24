use std::{
    collections::{BTreeMap, BTreeSet},
    iter::successors,
};

use ark_ff::PrimeField;
use ark_poly::{
    univariate::DensePolynomial, EvaluationDomain, GeneralEvaluationDomain,
    UVPolynomial,
};
use ark_std::rand::Rng;

use crate::{
    commitment::HomomorphicCommitment,
    data_structures::{IndexInfo, ProverPreprocessedInput, VerifierKey},
    lookup::{subset_equality::SubsetEqualityArgument, LookupArgument},
    oracles::{
        fixed::FixedProverOracle,
        instance::InstanceProverOracle,
        query::OracleType,
        rotation::Rotation,
        traits::{ConcreteOracle, Instantiable},
        witness::WitnessProverOracle,
    },
    permutation::PermutationArgument,
    piop::error::Error,
    piop::{verifier::VerifierFirstMsg, PIOPforPolyIdentity},
    vo::VirtualOracle,
};

use super::verifier::{VerifierLookupAggregationRound, VerifierPermutationMsg};

// Note: To keep flexible vanishing polynomial should not be strictly domain.vanishing_polynomial
// For example consider https://hackmd.io/1DaroFVfQwySwZPHMoMdBg?view where we remove some roots from Zh

// #[derive(Clone)]
/// State of the prover
pub struct ProverState<'a, F: PrimeField> {
    pub(crate) witness_oracles_mapping: BTreeMap<String, usize>, // TODO: introduce &str here maybe
    pub(crate) instance_oracles_mapping: BTreeMap<String, usize>,
    pub(crate) fixed_oracles_mapping: BTreeMap<String, usize>,
    pub(crate) table_oracles_mapping: BTreeMap<String, usize>,
    pub(crate) witness_oracles: &'a [WitnessProverOracle<F>],
    pub(crate) instance_oracles: &'a [InstanceProverOracle<F>],
    pub(crate) z_polys: Option<Vec<WitnessProverOracle<F>>>,
    pub(crate) oracles_to_copy: Vec<&'a WitnessProverOracle<F>>,
    pub(crate) vos: &'a [&'a dyn VirtualOracle<F>],
    pub(crate) domain: GeneralEvaluationDomain<F>,
    pub(crate) vanishing_polynomial: DensePolynomial<F>,
    pub(crate) quotient_chunks: Option<Vec<WitnessProverOracle<F>>>,
}

impl<F: PrimeField, PC: HomomorphicCommitment<F>> PIOPforPolyIdentity<F, PC> {
    pub fn init_prover<'a>(
        witness_oracles: &'a [WitnessProverOracle<F>],
        instance_oracles: &'a [InstanceProverOracle<F>],
        vos: &'a [&'a dyn VirtualOracle<F>],
        domain_size: usize,
        vanishing_polynomial: &DensePolynomial<F>,
        preprocessed: &ProverPreprocessedInput<F, PC>,
    ) -> ProverState<'a, F> {
        let domain = GeneralEvaluationDomain::new(domain_size).unwrap();

        let witness_oracles_mapping = witness_oracles
            .iter()
            .enumerate()
            .map(|(i, oracle)| (oracle.get_label(), i))
            .collect();
        let instance_oracles_mapping = instance_oracles
            .iter()
            .enumerate()
            .map(|(i, oracle)| (oracle.get_label(), i))
            .collect();
        let fixed_oracles_mapping = preprocessed
            .fixed_oracles
            .iter()
            .enumerate()
            .map(|(i, oracle)| (oracle.get_label(), i))
            .collect();
        let table_oracles_mapping = preprocessed
            .table_oracles
            .iter()
            .enumerate()
            .map(|(i, oracle)| (oracle.get_label(), i))
            .collect();

        let oracles_to_copy: Vec<&WitnessProverOracle<F>> = witness_oracles
            .iter()
            .filter(|&oracle| oracle.should_permute)
            .collect();

        ProverState {
            witness_oracles_mapping,
            instance_oracles_mapping,
            fixed_oracles_mapping,
            table_oracles_mapping,
            witness_oracles,
            instance_oracles,
            z_polys: None,
            oracles_to_copy,
            vos,
            domain,
            vanishing_polynomial: vanishing_polynomial.clone(),
            quotient_chunks: None,
        }
    }

    pub fn prover_permutation_round<'a, R: Rng>(
        permutation_msg: &VerifierPermutationMsg<F>,
        state: &mut ProverState<F>,
        permutation_argument: &PermutationArgument<F>,
        preprocessed: &ProverPreprocessedInput<F, PC>,
        extended_coset_domain: &GeneralEvaluationDomain<F>,
        zk_rng: &mut R,
    ) -> Vec<WitnessProverOracle<F>> {
        // if nothing to copy just return empty vector
        if state.oracles_to_copy.len() == 0 {
            state.z_polys = Some(vec![]);
            return vec![];
        }
        let z_polys = permutation_argument.construct_agg_polys(
            &state.oracles_to_copy,
            &preprocessed.permutation_oracles,
            permutation_msg.beta,
            permutation_msg.gamma,
            &state.domain,
            extended_coset_domain,
            zk_rng,
        );

        state.z_polys = Some(z_polys.clone());
        z_polys
    }

    pub fn prover_lookup_round<R: Rng>(
        lookup_aggregation_msg: &VerifierLookupAggregationRound<F>,
        state: &ProverState<F>,
        preprocessed: &ProverPreprocessedInput<F, PC>,
        index: &IndexInfo<F>,
        zk_rng: &mut R,
    ) -> Vec<(
        WitnessProverOracle<F>,
        WitnessProverOracle<F>,
        WitnessProverOracle<F>,
        WitnessProverOracle<F>,
    )> {
        let lookup_polys = index
            .lookups
            .iter()
            .enumerate()
            .map(|(lookup_index, lookup_vo)| {
                LookupArgument::construct_a_and_s_polys_prover(
                    &state.witness_oracles_mapping,
                    &state.instance_oracles_mapping,
                    &state.fixed_oracles_mapping,
                    &state.table_oracles_mapping,
                    &state.witness_oracles,
                    &state.instance_oracles,
                    &preprocessed.fixed_oracles,
                    &preprocessed.table_oracles,
                    1, // add usable rows in index
                    lookup_index,
                    lookup_vo.get_expressions(),
                    lookup_vo.get_table_queries(),
                    lookup_aggregation_msg.theta,
                    &state.domain,
                    &index.extended_coset_domain,
                    zk_rng,
                )
            })
            .collect();

        // In order: (a, s, a_prime, s_prime)
        lookup_polys
    }

    pub fn prover_lookup_subset_equality_round<R: Rng>(
        permutation_msg: &VerifierPermutationMsg<F>,
        // In order: (a, s, a_prime, s_prime)
        lookup_polys: &Vec<(
            WitnessProverOracle<F>,
            WitnessProverOracle<F>,
            WitnessProverOracle<F>,
            WitnessProverOracle<F>,
        )>,
        state: &ProverState<F>,
        index: &IndexInfo<F>,
        zk_rng: &mut R,
    ) -> Vec<WitnessProverOracle<F>> {
        let lookup_z_polys = lookup_polys
            .iter()
            .enumerate()
            .map(|(lookup_index, (a, s, a_prime, s_prime))| {
                SubsetEqualityArgument::construct_agg_poly(
                    a,
                    s,
                    a_prime,
                    s_prime,
                    permutation_msg.beta,
                    permutation_msg.gamma,
                    lookup_index,
                    1, //TODO: add u in index
                    &state.domain,
                    &index.extended_coset_domain,
                    zk_rng,
                )
            })
            .collect();
        lookup_z_polys
    }

    pub fn prover_quotient_round(
        verifier_permutation_msg: &VerifierPermutationMsg<F>,
        verifier_msg: &VerifierFirstMsg<F>,
        state: &mut ProverState<F>,
        vk: &VerifierKey<F, PC>,
        preprocessed: &ProverPreprocessedInput<F, PC>,
    ) -> Result<Vec<WitnessProverOracle<F>>, Error> {
        let powers_of_alpha: Vec<F> = successors(Some(F::one()), |alpha_i| {
            Some(*alpha_i * verifier_msg.alpha)
        })
        .take(state.vos.len())
        .collect();

        let mut numerator_evals =
            vec![F::zero(); vk.index_info.extended_coset_domain.size()];

        let z_polys = state.z_polys.as_ref().expect("Z polys are not in state");

        let number_of_alphas = vk
            .index_info
            .permutation_argument
            .number_of_alphas(z_polys.len());

        // start from next of last power of alpha
        let begin_with =
            powers_of_alpha.last().unwrap().clone() * verifier_msg.alpha;
        let permutation_alphas: Vec<F> =
            successors(Some(begin_with), |alpha_i| {
                Some(*alpha_i * verifier_msg.alpha)
            })
            .take(number_of_alphas)
            .collect();

        let domain_size = state.domain.size();
        let mut l0_evals = vec![F::zero(); domain_size];
        l0_evals[0] = F::one();
        let l0 = DensePolynomial::from_coefficients_slice(
            &state.domain.ifft(&l0_evals),
        );
        let l0_coset_evals = vk.index_info.extended_coset_domain.coset_fft(&l0);

        let mut lu_evals = vec![F::zero(); domain_size];
        lu_evals[vk.index_info.permutation_argument.u] = F::one();
        let lu = DensePolynomial::from_coefficients_slice(
            &state.domain.ifft(&lu_evals),
        );
        let lu_coset_evals = vk.index_info.extended_coset_domain.coset_fft(&lu);

        for i in 0..vk.index_info.extended_coset_domain.size() {
            for (vo_index, vo) in state.vos.iter().enumerate() {
                let vo_evaluation = vo.get_expression().evaluate(
                    &|x: F| x,
                    &|query| {
                        match query.oracle_type {
                            OracleType::Witness => {
                                match state.witness_oracles_mapping.get(&query.label) {
                                    Some(index) => state.witness_oracles[*index].query_in_coset(i, query.rotation),
                                    None => panic!("Witness oracle with label add_label not found") //TODO: Introduce new Error here,
                                }
                            },
                            OracleType::Instance => {
                                match state.instance_oracles_mapping.get(&query.label) {
                                    Some(index) => state.instance_oracles[*index].query_in_coset(i, query.rotation),
                                    None => panic!("Instance oracle with label add_label not found") //TODO: Introduce new Error here,
                                }
                            },
                            OracleType::Fixed => {
                                match state.fixed_oracles_mapping.get(&query.label) {
                                    Some(index) => preprocessed.fixed_oracles[*index].query_in_coset(i, query.rotation),
                                    None => panic!("Fixed oracle with label add_label not found") //TODO: Introduce new Error here,
                                }
                            },
                        }
                    },
                    &|x: F| -x,
                    &|x: F, y: F| x + y,
                    &|x: F, y: F| x * y,
                    &|x: F, y: F| x * y,
                );

                numerator_evals[i] += powers_of_alpha[vo_index] * vo_evaluation;
            }

            // Permutation argument
            // If there are no oracles to enforce copy constraints on, we just return zero
            numerator_evals[i] += if state.oracles_to_copy.len() > 0 {
                vk.index_info
                    .permutation_argument
                    .instantiate_argument_at_omega_i(
                        &l0_coset_evals,
                        &lu_coset_evals,
                        &preprocessed.q_blind,
                        &state.oracles_to_copy,
                        &preprocessed.permutation_oracles,
                        &z_polys,
                        i,
                        vk.index_info.extended_coset_domain.element(i),
                        verifier_permutation_msg.beta,
                        verifier_permutation_msg.gamma,
                        &state.domain,
                        &permutation_alphas,
                    )
            } else {
                F::zero()
            };
        }

        let quotient_evals: Vec<_> = numerator_evals
            .iter()
            .zip(vk.zh_inverses_over_coset.iter())
            .map(|(&nom, &denom)| nom * denom)
            .collect();

        let quotient = DensePolynomial::from_coefficients_slice(
            &vk.index_info
                .extended_coset_domain
                .coset_ifft(&quotient_evals),
        );

        let mut quotient_coeffs = Vec::from(quotient.coeffs());
        let padding_size =
            vk.index_info.extended_coset_domain.size() - quotient_coeffs.len();
        if padding_size > 0 {
            quotient_coeffs.extend(vec![F::zero(); padding_size]);
        }

        let quotient_chunks: Vec<WitnessProverOracle<F>> = quotient_coeffs
            .chunks(state.domain.size())
            .enumerate()
            .map(|(i, chunk)| {
                let poly = DensePolynomial::from_coefficients_slice(chunk);
                WitnessProverOracle {
                    label: format!("quotient_chunk_{}", i).to_string(),
                    evals: state.domain.fft(&poly),
                    poly,
                    evals_at_coset_of_extended_domain: None,
                    queried_rotations: BTreeSet::from([Rotation::curr()]),
                    should_permute: false,
                }
            })
            .collect();

        state.quotient_chunks = Some(quotient_chunks.clone());
        Ok(quotient_chunks)
    }
}

#[cfg(test)]
mod test {
    use crate::commitment::{HomomorphicCommitment, KZG10};
    use ark_bls12_381::{Bls12_381, Fr as F};
    use ark_ff::{One, UniformRand};
    use ark_poly::{univariate::DensePolynomial, Polynomial, UVPolynomial};
    use ark_poly_commit::{
        LabeledCommitment, LabeledPolynomial, PCUniversalParams,
        PolynomialCommitment,
    };
    use ark_std::test_rng;

    type PC = KZG10<Bls12_381>;

    #[test]
    fn test_rands_homomorphism() {
        let max_degree = 17;
        let mut rng = test_rng();

        let poly_degree = 10;
        let srs = PC::setup(max_degree, None, &mut rng).unwrap();

        let (committer_key, verifier_key) =
            PC::trim(&srs, srs.max_degree(), 1, None).unwrap();

        let p = DensePolynomial::<F>::rand(poly_degree, &mut rng);
        let p =
            LabeledPolynomial::new("p".to_string(), p.clone(), None, Some(1));
        let q = DensePolynomial::<F>::rand(poly_degree, &mut rng);
        let q =
            LabeledPolynomial::new("q".to_string(), q.clone(), None, Some(1));

        let polys = [p.clone(), q.clone()];

        let (comms, rands) =
            PC::commit(&committer_key, polys.iter(), Some(&mut rng)).unwrap();

        let challenge = F::rand(&mut rng);
        let x1 = F::rand(&mut rng);

        let r = p.polynomial().clone() + q.polynomial() * x1;
        let value = r.evaluate(&challenge);

        let r =
            LabeledPolynomial::new("r".to_string(), r.clone(), None, Some(1));

        let r_comm = PC::add(
            comms[0].commitment(),
            &PC::scale_com(comms[1].commitment(), x1),
        );
        let r_comm =
            LabeledCommitment::new("r_comm".to_string(), r_comm.clone(), None);
        let r_rand = PC::add_rands(&rands[0], &PC::scale_rand(&rands[1], x1));

        let proof = PC::open(
            &committer_key,
            &[r.clone()],
            &[r_comm],
            &challenge,
            F::one(),
            &[r_rand],
            Some(&mut rng),
        )
        .unwrap();

        let r_comm = PC::add(
            comms[0].commitment(),
            &PC::scale_com(comms[1].commitment(), x1),
        );
        let r_comm =
            LabeledCommitment::new("r_comm".to_string(), r_comm.clone(), None);

        let res = PC::check(
            &verifier_key,
            &[r_comm.clone()],
            &challenge,
            vec![value],
            &proof,
            F::one(),
            Some(&mut rng),
        )
        .unwrap();
        assert_eq!(res, true);
    }
}
