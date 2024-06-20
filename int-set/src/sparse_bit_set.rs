//! Provides serialization of IntSet's to a highly compact bitset format as defined in the
//! IFT specification:
//!
//! https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding

use std::collections::VecDeque;

use crate::input_bit_stream::InputBitStream;
use crate::output_bit_stream::OutputBitStream;
use crate::IntSet;
use thiserror::Error;

#[derive(Error, Debug)]
#[error("The input data stream was too short to be a valid sparse bit set.")]
pub struct DecodingError();

pub enum BranchFactor {
    Two,
    Four,
    Eight,
    ThirtyTwo,
}

// TODO eliminate cases of explicitly provding BF (eg. ::<2>)

pub(crate) fn to_sparse_bit_set(set: &IntSet<u32>) -> Vec<u8> {
    // TODO(garretrieger): use the heuristic approach from the incxfer
    // implementation to guess the optimal size. Building the set 4 times
    // is costly.
    // TODO: skip BF's that can't be used due to exceeding max height.
    // TODO: for loop?
    // TODO: const array with all of the valid BF values.
    let candidates: Vec<Vec<u8>> = vec![
        to_sparse_bit_set_internal::<2>(set),
        to_sparse_bit_set_internal::<4>(set),
        to_sparse_bit_set_internal::<8>(set),
        to_sparse_bit_set_internal::<32>(set),
    ];

    candidates.into_iter().min_by_key(|f| f.len()).unwrap()
}

pub(crate) fn to_sparse_bit_set_with_bf(set: &IntSet<u32>, branch_factor: BranchFactor) -> Vec<u8> {
    match branch_factor {
        BranchFactor::Two => to_sparse_bit_set_internal::<2>(set),
        BranchFactor::Four => to_sparse_bit_set_internal::<4>(set),
        BranchFactor::Eight => to_sparse_bit_set_internal::<8>(set),
        BranchFactor::ThirtyTwo => to_sparse_bit_set_internal::<32>(set),
    }
}

fn to_sparse_bit_set_internal<const BF: u32>(set: &IntSet<u32>) -> Vec<u8> {
    // TODO(garretrieger): implement detection of filled nodes (ie. zero nodes)
    let Some(max_value) = set.last() else {
        return OutputBitStream::<BF>::new(0).into_bytes();
    };
    let mut height = tree_height_for(BF, max_value);
    let mut os = OutputBitStream::<BF>::new(height);
    let mut nodes: Vec<Node> = vec![];

    // We built the nodes that will comprise the bit stream in reverse order
    // from the last value in the last layer up to the first layer. Then
    // when generating the final stream the order is reversed.
    // The reverse order construction is needed since nodes at the lower layer
    // affect the values in the parent layers.
    let mut indices = set.clone();
    while height > 0 {
        indices = create_layer(BF, indices.iter(), &mut nodes);
        height -= 1;
    }

    for node in nodes.iter().rev() {
        os.write_node(node.bits);
    }

    os.into_bytes()
}

/// Compute the nodes for a layer of the sparse bit set.
///
/// Computes the nodes needed for the layer which contains the indices in
/// 'iter'. The new nodes are appeded to 'nodes'. 'iter' must be sorted
/// in ascending order.
///
/// Returns the set of indices for the layer above.
fn create_layer<T: DoubleEndedIterator<Item = u32>>(
    branch_factor: u32,
    iter: T,
    nodes: &mut Vec<Node>,
) -> IntSet<u32> {
    let mut next_indices = IntSet::<u32>::empty();

    // The nodes array is produced in reverse order and then reversed before final output.
    let mut current_node: Option<Node> = None;
    for v in iter.rev() {
        let parent_index = v / branch_factor;
        let prev_parent_index = current_node
            .as_ref()
            .map_or(parent_index, |node| node.parent_index);
        if prev_parent_index != parent_index {
            nodes.push(current_node.take().unwrap());
            next_indices.insert(prev_parent_index);
        }

        let current_node = current_node.get_or_insert(Node {
            bits: 0,
            parent_index,
        });

        current_node.bits |= 0b1 << (v % branch_factor);
    }
    if let Some(node) = current_node {
        next_indices.insert(node.parent_index);
        nodes.push(node);
    }

    next_indices
}

struct Node {
    bits: u32,
    parent_index: u32,
}

fn tree_height_for(branch_factor: u32, max_value: u32) -> u8 {
    // height H, can represent up to (BF^height) - 1
    let mut height: u32 = 0;
    let mut max_value = max_value;
    loop {
        height += 1;
        max_value >>= branch_factor_node_size_log2(branch_factor);
        if max_value == 0 {
            break height as u8;
        }
    }
}

fn branch_factor_node_size_log2(branch_factor: u32) -> u32 {
    match branch_factor {
        2 => 1,
        4 => 2,
        8 => 3,
        32 => 5,
        // TODO(garretrieger): convert the int constant to an enum value and
        //   match on that, then panic is only needed during the conversion.
        _ => panic!("Invalid branch factor."),
    }
}

struct NextNode {
    start: u32,
    depth: u32,
}

pub(crate) fn from_sparse_bit_set(data: &[u8]) -> Result<IntSet<u32>, DecodingError> {
    // This is a direct port of the decoding algorithm from:
    // https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding
    let mut bits = InputBitStream::from(data);

    let Some(branch_factor) = bits.read_branch_factor() else {
        return Err(DecodingError());
    };

    let Some(height) = bits.read_height() else {
        return Err(DecodingError());
    };

    let mut out = IntSet::<u32>::empty();
    if height == 0 {
        return Ok(out);
    }

    // Bit 8 of header byte is ignored.
    bits.skip_bit();

    let mut queue = VecDeque::<NextNode>::new(); // TODO(garretrieger): estimate initial capacity?
    queue.push_back(NextNode { start: 0, depth: 1 });

    while let Some(next) = queue.pop_front() {
        let mut has_a_one = false;
        for index in 0..branch_factor as u32 {
            let Some(bit) = bits.read_bit() else {
                return Err(DecodingError());
            };

            if !bit {
                continue;
            }

            // TODO(garretrieger): use two while loops (one for non-leaf and one for leaf nodes)
            //                     to avoid having to branch on each iteration.
            has_a_one = true;
            if next.depth == height as u32 {
                // TODO(garretrieger): optimize insertion speed by using the bulk sorted insert
                // (rewrite this to be an iterator) or even directly writing groups of bits to the pages.
                out.insert(next.start + index);
            } else {
                let exp = height as u32 - next.depth;
                queue.push_back(NextNode {
                    start: next.start + index * (branch_factor as u32).pow(exp),
                    depth: next.depth + 1,
                });
            }
        }

        if !has_a_one {
            // all bits were zeroes which is a special command to completely fill in
            // all integers covered by this node.
            let exp = (height as u32) - next.depth + 1;
            out.insert_range(next.start..=next.start + (branch_factor as u32).pow(exp) - 1);
        }
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unusual_byte_groupings)]
mod test {
    use super::*;

    #[test]
    fn spec_example_2() {
        // Test of decoding the example 2 given in the specification.
        // See: https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding
        let bytes = [
            0b00001110, 0b00100001, 0b00010001, 0b00000001, 0b00000100, 0b00000010, 0b00001000,
        ];

        let set = from_sparse_bit_set(&bytes).unwrap();
        let expected: IntSet<u32> = [2, 33, 323].iter().copied().collect();
        assert_eq!(set, expected);
    }

    #[test]
    fn spec_example_3() {
        // Test of decoding the example 3 given in the specification.
        // See: https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding
        let bytes = [0b00000000];

        let set = from_sparse_bit_set(&bytes).unwrap();
        let expected: IntSet<u32> = [].iter().copied().collect();
        assert_eq!(set, expected);
    }

    #[test]
    fn spec_example_4() {
        // Test of decoding the example 4 given in the specification.
        // See: https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding
        let bytes = [0b00001101, 0b00000011, 0b00110001];

        let set = from_sparse_bit_set(&bytes).unwrap();

        let mut expected: IntSet<u32> = IntSet::<u32>::empty();
        expected.insert_range(0..=17);

        assert_eq!(set, expected);
    }

    #[test]
    fn invalid() {
        // Spec example 2 with one byte missing.
        let bytes = [
            0b00001110, 0b00100001, 0b00010001, 0b00000001, 0b00000100, 0b00000010,
        ];
        assert!(from_sparse_bit_set(&bytes).is_err());
    }

    #[test]
    fn test_tree_height_for() {
        assert_eq!(tree_height_for(2, 0), 1);
        assert_eq!(tree_height_for(2, 1), 1);
        assert_eq!(tree_height_for(2, 2), 2);
        assert_eq!(tree_height_for(2, 117), 7);

        assert_eq!(tree_height_for(4, 0), 1);
        assert_eq!(tree_height_for(4, 3), 1);
        assert_eq!(tree_height_for(4, 4), 2);
        assert_eq!(tree_height_for(4, 63), 3);
        assert_eq!(tree_height_for(4, 64), 4);

        assert_eq!(tree_height_for(8, 0), 1);
        assert_eq!(tree_height_for(8, 7), 1);
        assert_eq!(tree_height_for(8, 8), 2);
        assert_eq!(tree_height_for(8, 32767), 5);
        assert_eq!(tree_height_for(8, 32768), 6);

        assert_eq!(tree_height_for(32, 0), 1);
        assert_eq!(tree_height_for(32, 31), 1);
        assert_eq!(tree_height_for(32, 32), 2);
        assert_eq!(tree_height_for(32, 1_048_575), 4);
        assert_eq!(tree_height_for(32, 1_048_576), 5);
    }

    #[test]
    fn generate_spec_example_2() {
        // Test of reproducing the encoding of example 2 given
        // in the specification. See:
        // https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding

        let actual_bytes =
            to_sparse_bit_set_with_bf(&[2, 33, 323].iter().copied().collect(), BranchFactor::Eight);
        let expected_bytes = [
            0b00001110, 0b00100001, 0b00010001, 0b00000001, 0b00000100, 0b00000010, 0b00001000,
        ];

        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test]
    fn generate_spec_example_3() {
        // Test of reproducing the encoding of example 3 given
        // in the specification. See:
        // https://w3c.github.io/IFT/Overview.html#sparse-bit-set-decoding

        let actual_bytes = to_sparse_bit_set_with_bf(&IntSet::<u32>::empty(), BranchFactor::Two);
        let expected_bytes = [0b00000000];

        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test]
    fn encode_bf32() {
        let actual_bytes = to_sparse_bit_set_with_bf(
            &[2, 31, 323].iter().copied().collect(),
            BranchFactor::ThirtyTwo,
        );
        let expected_bytes = [
            0b0_00010_11,
            // node 0
            0b00000001,
            0b00000100,
            0b00000000,
            0b00000000,
            // node 1
            0b00000100,
            0b00000000,
            0b00000000,
            0b10000000,
            // node 2
            0b00001000,
            0b00000000,
            0b00000000,
            0b00000000,
        ];

        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test]
    fn round_trip() {
        let s1: IntSet<u32> = [11, 74, 9358].iter().copied().collect();
        let mut s2: IntSet<u32> = s1.clone();
        s2.insert_range(67..=412);

        check_round_trip(&s1, BranchFactor::Two);
        check_round_trip(&s1, BranchFactor::Four);
        check_round_trip(&s1, BranchFactor::Eight);
        check_round_trip(&s1, BranchFactor::ThirtyTwo);

        check_round_trip(&s2, BranchFactor::Two);
        check_round_trip(&s2, BranchFactor::Four);
        check_round_trip(&s2, BranchFactor::Eight);
        check_round_trip(&s2, BranchFactor::ThirtyTwo);
    }

    fn check_round_trip(s: &IntSet<u32>, branch_factor: BranchFactor) {
        let bytes = to_sparse_bit_set_with_bf(s, branch_factor);
        let s_prime = from_sparse_bit_set(&bytes).unwrap();
        assert_eq!(*s, s_prime);
    }

    #[test]
    fn find_smallest_bf() {
        let s: IntSet<u32> = [11, 74, 9358].iter().copied().collect();
        let bytes = to_sparse_bit_set(&s);
        // BF4
        assert_eq!(vec![0b0_00111_01], bytes[0..1]);

        let s: IntSet<u32> = [
            16, 0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30,
        ]
        .iter()
        .copied()
        .collect();
        let bytes = to_sparse_bit_set(&s);
        // BF32
        assert_eq!(vec![0b0_00001_11], bytes[0..1]);
    }
}
