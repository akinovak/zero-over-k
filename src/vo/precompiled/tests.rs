use ark_bls12_381::{Bls12_381, Fr};
use ark_ff::Zero;
use ark_poly::{
    univariate::DensePolynomial, EvaluationDomain, GeneralEvaluationDomain,
    UVPolynomial,
};

use ark_poly_commit::sonic_pc::CommitterKey;
use ark_poly_commit::sonic_pc::VerifierKey;
use ark_poly_commit::{LabeledPolynomial, PolynomialCommitment};
use ark_serialize::CanonicalSerialize;
use ark_std::rand::rngs::StdRng;
use ark_std::test_rng;
use rand_chacha::ChaChaRng;

use crate::data_structures::{
    PermutationInfo, Proof, ProverKey, ProverPreprocessedInput,
    VerifierPreprocessedInput,
};
use crate::indexer::Indexer;
use crate::oracles::fixed::{FixedProverOracle, FixedVerifierOracle};
use crate::oracles::instance::{InstanceProverOracle, InstanceVerifierOracle};

use crate::oracles::witness::{WitnessProverOracle, WitnessVerifierOracle};
use crate::rng::SimpleHashFiatShamirRng;
use crate::vo::generic_vo::GenericVO;
use crate::PIL;
use blake2::Blake2s;

use crate::commitment::{HomomorphicCommitment, KZG10};
use crate::vo::VirtualOracle;

type F = Fr;
type FS = SimpleHashFiatShamirRng<Blake2s, ChaChaRng>;
type PC = KZG10<Bls12_381>;

type CommKey = CommitterKey<Bls12_381>;
type CSVerKey = VerifierKey<Bls12_381>;

type PilInstance = PIL<F, PC, FS>;

/// Initialize domain, srs and CS keys
pub fn test_init(
    domain_size: usize,
) -> (GeneralEvaluationDomain<F>, (CommKey, CSVerKey), StdRng) {
    let poly_degree = domain_size - 1;
    let max_degree = poly_degree; // + max_blinding;

    let mut rng = test_rng();
    let srs = PilInstance::universal_setup(max_degree, &mut rng).unwrap();

    let cs_keys = PilInstance::prepare_keys(&srs).unwrap();

    let domain = GeneralEvaluationDomain::<F>::new(domain_size).unwrap();
    (domain, cs_keys, rng)
}

/// Run a prover to test a VO without using copy constraints or lookups
pub(crate) fn run_prover(
    domain: GeneralEvaluationDomain<F>,
    ck: CommKey,
    cs_vk: CSVerKey,
    witness: Vec<(impl Into<String>, &[F])>,
    fixed: Vec<(impl Into<String>, &[F])>,
    instance: Vec<(impl Into<String>, &[F])>,
    mut vo: GenericVO<F>,
    rng: &mut StdRng,
) -> Proof<F, PC> {
    // 1. Generate Prover Oracles
    let mut witness_oracles_raw: Vec<WitnessProverOracle<F>> = Vec::with_capacity(witness.len());
    let mut witness_oracles_refs: Vec<&mut WitnessProverOracle<F>> = Vec::with_capacity(witness.len());
    for (label, evals) in witness {
        let poly = DensePolynomial::from_coefficients_slice(&domain.ifft(&evals));
        let w = WitnessProverOracle::new(label, poly, &evals, false);
        witness_oracles_raw.push(w);
    }
    for w in witness_oracles_raw.iter_mut() {
        witness_oracles_refs.push(w);
    }

    let mut witness_oracles: &mut [&mut WitnessProverOracle<F>] = &mut witness_oracles_refs;

    let mut fixed_oracles_raw: Vec<FixedProverOracle<F>> = fixed
        .into_iter()
        .map(|(label, evals)| {
            let poly =
                DensePolynomial::from_coefficients_slice(&domain.ifft(&evals));
            FixedProverOracle::new(label, poly, &evals)
        })
        .collect();

    let mut fixed_oracles_v: Vec<&mut FixedProverOracle<F>> = fixed_oracles_raw
        .iter_mut()
        .collect();

    let mut fixed_oracles: &mut [&mut FixedProverOracle<F>] = fixed_oracles_v.iter_mut().into_slice();

    //let mut fixed_oracles: &mut [&mut FixedProverOracle<F>] = fixed.

    let mut instance_oracles: Vec<_> = instance
        .into_iter()
        .map(|(label, evals)| {
            let poly =
                DensePolynomial::from_coefficients_slice(&domain.ifft(&evals));
            InstanceProverOracle::new(label, poly, &evals)
        })
        .collect();

    // 2. Configure VO
    vo.configure(
        &mut witness_oracles,
        &mut instance_oracles,
        &mut fixed_oracles,
    );

    // 3. Generate pk and vk

    let vos: Vec<&dyn VirtualOracle<F>> = vec![&vo];

    let vk: crate::data_structures::VerifierKey<F, PC> = Indexer::index(
        &cs_vk,
        &vos,
        vec![],
        witness_oracles,
        &instance_oracles,
        &fixed_oracles,
        domain,
        &domain.vanishing_polynomial().into(),
        PermutationInfo::empty(),
        0,
    )
    .unwrap();

    let pk = ProverKey::from_ck_and_vk(&ck, &vk);


    // 4. Generate Prover precoessed input

    let q_blind =
        FixedProverOracle::new("q_blind", DensePolynomial::zero(), &[]);

    let mut preprocessed = ProverPreprocessedInput::new(
        &mut fixed_oracles,
        &vec![],
        &vec![],
        &q_blind,
        &vk.index_info,
    );

    // 5. Prove

    let proof = PilInstance::prove(
        &pk,
        &mut preprocessed,
        &mut witness_oracles,
        &mut instance_oracles,
        &vos,
        domain.size(),
        &domain.vanishing_polynomial().into(),
        rng,
    )
    .unwrap();

    // 6. Print Proof info

    println!("{}", proof.info());
    println!("{}", proof.cumulative_info());
    println!("in bytes: {}", proof.serialized_size());

    proof
}

/// Run a verifier to test a VO without using copy constraints or lookups
pub(crate) fn run_verifier(
    domain: GeneralEvaluationDomain<F>,
    ck: CommKey,
    cs_vk: CSVerKey,
    witness_labels: Vec<impl Into<String>>,
    fixed: Vec<(impl Into<String>, &[F])>,
    instance: Vec<(impl Into<String>, &[F])>,
    mut vo: GenericVO<F>,
    proof: Proof<F, PC>,
    rng: &mut StdRng,
) {
    // 1. Generate Verifier Oracles
    let mut witness_oracles_raw: Vec<WitnessVerifierOracle<F, PC>> = Vec::with_capacity(witness_labels.len());
    let mut witness_oracles_refs: Vec<&mut WitnessVerifierOracle<F, PC>> = Vec::with_capacity(witness_labels.len());
    for label in witness_labels {
        let w = WitnessVerifierOracle::new(label, false);
        witness_oracles_raw.push(w);
    }
    for w in witness_oracles_raw.iter_mut() {
        witness_oracles_refs.push(w);
    }

    let mut witness_ver_oracles: &mut [&mut WitnessVerifierOracle<F, PC>] = &mut witness_oracles_refs;

    let mut instance_oracles: Vec<InstanceVerifierOracle<F>> = instance
        .into_iter()
        .map(|(label, evals)| {
            let poly =
                DensePolynomial::from_coefficients_slice(&domain.ifft(&evals));
            InstanceVerifierOracle::new(label, poly, &evals)
        })
        .collect();

    let labeled_fixed: Vec<LabeledPolynomial<F, DensePolynomial<F>>> = fixed
        .into_iter()
        .map(|(label, evals)| {
            let poly =
                DensePolynomial::from_coefficients_slice(&domain.ifft(&evals));
            LabeledPolynomial::new(label.into(), poly, None, None)
        })
        .collect();

    let (fixed_comm, _) = PC::commit(&ck, labeled_fixed.iter(), None).unwrap();

    let mut fixed_oracles_raw: Vec<FixedVerifierOracle<F, PC>> = fixed_comm
        .into_iter()
        .map(|comm| FixedVerifierOracle::from_commitment(comm))
        .collect();

    let mut fixed_oracles_v: Vec<&mut FixedVerifierOracle<F, PC>> = fixed_oracles_raw
        .iter_mut()
        .collect();
    
    let mut fixed_oracles: &mut [&mut FixedVerifierOracle<F, PC>] = fixed_oracles_v.iter_mut().into_slice();

    // 2. Configure VO
    vo.configure(
        &mut witness_ver_oracles,
        &mut instance_oracles,
        &mut fixed_oracles,
    );

    // 3. Generate verifier's preprocessed
    let vos: Vec<&dyn VirtualOracle<F>> = vec![&vo];

    let mut vk = Indexer::index(
        &cs_vk,
        &vos,
        vec![],
        &witness_ver_oracles,
        &instance_oracles,
        &mut fixed_oracles,
        domain,
        &domain.vanishing_polynomial().into(),
        PermutationInfo::empty(),
        0,
    )
    .unwrap();

    let q_blind = FixedVerifierOracle::new("q_blind".to_string(), Some(PC::zero_comm()));
    let mut preprocessed = VerifierPreprocessedInput {
        fixed_oracles,
        table_oracles: vec![],
        permutation_oracles: vec![],
        q_blind,
    };

    // 4. Verify proof

    let res = PilInstance::verify(
        &mut vk,
        &mut preprocessed,
        proof,
        &mut witness_ver_oracles,
        &mut instance_oracles,
        vos.as_slice(),
        domain.size(),
        &domain.vanishing_polynomial().into(),
        rng,
    )
    .unwrap();

    res
}
