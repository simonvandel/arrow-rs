// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Defines aggregations over Arrow arrays.

use arrow_array::cast::*;
use arrow_array::iterator::ArrayIter;
use arrow_array::*;
use arrow_buffer::ArrowNativeType;
use arrow_data::bit_iterator::try_for_each_valid_idx;
use arrow_schema::ArrowError;
use arrow_schema::*;
use std::ops::{BitAnd, BitOr, BitXor};

/// Generic test for NaN, the optimizer should be able to remove this for integer types.
#[inline]
pub(crate) fn is_nan<T: ArrowNativeType + PartialOrd + Copy>(a: T) -> bool {
    #[allow(clippy::eq_op)]
    !(a == a)
}

/// Returns the minimum value in the array, according to the natural order.
/// For floating point arrays any NaN values are considered to be greater than any other non-null value
#[cfg(not(feature = "simd"))]
pub fn min<T>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeType,
{
    min_max_helper::<T::Native, _, _>(array, |a, b| (is_nan(*a) & !is_nan(*b)) || a > b)
}

/// Returns the maximum value in the array, according to the natural order.
/// For floating point arrays any NaN values are considered to be greater than any other non-null value
#[cfg(not(feature = "simd"))]
pub fn max<T>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeType,
{
    min_max_helper::<T::Native, _, _>(array, |a, b| (!is_nan(*a) & is_nan(*b)) || a < b)
}

/// Returns the minimum value in the boolean array.
///
/// ```
/// # use arrow_array::BooleanArray;
/// # use arrow_arith::aggregate::min_boolean;
///
/// let a = BooleanArray::from(vec![Some(true), None, Some(false)]);
/// assert_eq!(min_boolean(&a), Some(false))
/// ```
pub fn min_boolean(array: &BooleanArray) -> Option<bool> {
    // short circuit if all nulls / zero length array
    if array.null_count() == array.len() {
        return None;
    }

    // Note the min bool is false (0), so short circuit as soon as we see it
    array
        .iter()
        .find(|&b| b == Some(false))
        .flatten()
        .or(Some(true))
}

/// Returns the maximum value in the boolean array
///
/// ```
/// # use arrow_array::BooleanArray;
/// # use arrow_arith::aggregate::max_boolean;
///
/// let a = BooleanArray::from(vec![Some(true), None, Some(false)]);
/// assert_eq!(max_boolean(&a), Some(true))
/// ```
pub fn max_boolean(array: &BooleanArray) -> Option<bool> {
    // short circuit if all nulls / zero length array
    if array.null_count() == array.len() {
        return None;
    }

    // Note the max bool is true (1), so short circuit as soon as we see it
    array
        .iter()
        .find(|&b| b == Some(true))
        .flatten()
        .or(Some(false))
}

/// Helper to compute min/max of [`ArrayAccessor`].
fn min_max_helper<T, A: ArrayAccessor<Item = T>, F>(array: A, cmp: F) -> Option<T>
where
    F: Fn(&T, &T) -> bool,
{
    let null_count = array.null_count();
    if null_count == array.len() {
        None
    } else if null_count == 0 {
        // JUSTIFICATION
        //  Benefit:  ~8% speedup
        //  Soundness: `i` is always within the array bounds
        (0..array.len())
            .map(|i| unsafe { array.value_unchecked(i) })
            .reduce(|acc, item| if cmp(&acc, &item) { item } else { acc })
    } else {
        let nulls = array.nulls().unwrap();
        unsafe {
            let idx = nulls.valid_indices().reduce(|acc_idx, idx| {
                let acc = array.value_unchecked(acc_idx);
                let item = array.value_unchecked(idx);
                if cmp(&acc, &item) {
                    idx
                } else {
                    acc_idx
                }
            });
            idx.map(|idx| array.value_unchecked(idx))
        }
    }
}

/// Returns the maximum value in the binary array, according to the natural order.
pub fn max_binary<T: OffsetSizeTrait>(array: &GenericBinaryArray<T>) -> Option<&[u8]> {
    min_max_helper::<&[u8], _, _>(array, |a, b| *a < *b)
}

/// Returns the minimum value in the binary array, according to the natural order.
pub fn min_binary<T: OffsetSizeTrait>(array: &GenericBinaryArray<T>) -> Option<&[u8]> {
    min_max_helper::<&[u8], _, _>(array, |a, b| *a > *b)
}

/// Returns the maximum value in the string array, according to the natural order.
pub fn max_string<T: OffsetSizeTrait>(array: &GenericStringArray<T>) -> Option<&str> {
    min_max_helper::<&str, _, _>(array, |a, b| *a < *b)
}

/// Returns the minimum value in the string array, according to the natural order.
pub fn min_string<T: OffsetSizeTrait>(array: &GenericStringArray<T>) -> Option<&str> {
    min_max_helper::<&str, _, _>(array, |a, b| *a > *b)
}

/// Returns the sum of values in the array.
///
/// This doesn't detect overflow. Once overflowing, the result will wrap around.
/// For an overflow-checking variant, use `sum_array_checked` instead.
pub fn sum_array<T, A: ArrayAccessor<Item = T::Native>>(array: A) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeTypeOp,
{
    match array.data_type() {
        DataType::Dictionary(_, _) => {
            let null_count = array.null_count();

            if null_count == array.len() {
                return None;
            }

            let iter = ArrayIter::new(array);
            let sum = iter
                .into_iter()
                .fold(T::default_value(), |accumulator, value| {
                    if let Some(value) = value {
                        accumulator.add_wrapping(value)
                    } else {
                        accumulator
                    }
                });

            Some(sum)
        }
        _ => sum::<T>(as_primitive_array(&array)),
    }
}

/// Returns the sum of values in the array.
///
/// This detects overflow and returns an `Err` for that. For an non-overflow-checking variant,
/// use `sum_array` instead.
pub fn sum_array_checked<T, A: ArrayAccessor<Item = T::Native>>(
    array: A,
) -> Result<Option<T::Native>, ArrowError>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeTypeOp,
{
    match array.data_type() {
        DataType::Dictionary(_, _) => {
            let null_count = array.null_count();

            if null_count == array.len() {
                return Ok(None);
            }

            let iter = ArrayIter::new(array);
            let sum =
                iter.into_iter()
                    .try_fold(T::default_value(), |accumulator, value| {
                        if let Some(value) = value {
                            accumulator.add_checked(value)
                        } else {
                            Ok(accumulator)
                        }
                    })?;

            Ok(Some(sum))
        }
        _ => sum_checked::<T>(as_primitive_array(&array)),
    }
}

/// Returns the min of values in the array of `ArrowNumericType` type, or dictionary
/// array with value of `ArrowNumericType` type.
pub fn min_array<T, A: ArrayAccessor<Item = T::Native>>(array: A) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeType,
{
    min_max_array_helper::<T, A, _, _>(
        array,
        |a, b| (is_nan(*a) & !is_nan(*b)) || a > b,
        min,
    )
}

/// Returns the max of values in the array of `ArrowNumericType` type, or dictionary
/// array with value of `ArrowNumericType` type.
pub fn max_array<T, A: ArrayAccessor<Item = T::Native>>(array: A) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeType,
{
    min_max_array_helper::<T, A, _, _>(
        array,
        |a, b| (!is_nan(*a) & is_nan(*b)) || a < b,
        max,
    )
}

fn min_max_array_helper<T, A: ArrayAccessor<Item = T::Native>, F, M>(
    array: A,
    cmp: F,
    m: M,
) -> Option<T::Native>
where
    T: ArrowNumericType,
    F: Fn(&T::Native, &T::Native) -> bool,
    M: Fn(&PrimitiveArray<T>) -> Option<T::Native>,
{
    match array.data_type() {
        DataType::Dictionary(_, _) => min_max_helper::<T::Native, _, _>(array, cmp),
        _ => m(as_primitive_array(&array)),
    }
}

/// Returns the sum of values in the primitive array.
///
/// Returns `None` if the array is empty or only contains null values.
///
/// This doesn't detect overflow. Once overflowing, the result will wrap around.
/// For an overflow-checking variant, use `sum_checked` instead.
#[cfg(not(feature = "simd"))]
pub fn sum<T>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeTypeOp,
{
    let null_count = array.null_count();

    if null_count == array.len() {
        return None;
    }

    let data: &[T::Native] = array.values();
    // TODO choose lanes based on T::Native. Extract from simd module
    const LANES: usize = 16;
    let mut chunk_acc = [T::default_value(); LANES];
    let mut rem_acc = T::default_value();

    match array.nulls() {
        None => {
            let data_chunks = data.chunks_exact(64);
            let remainder = data_chunks.remainder();

            data_chunks.for_each(|chunk| {
                chunk.chunks_exact(LANES).for_each(|chunk| {
                    let chunk: [T::Native; LANES] = chunk.try_into().unwrap();

                    for i in 0..LANES {
                        chunk_acc[i] = chunk_acc[i].add_wrapping(chunk[i]);
                    }
                })
            });

            remainder.iter().copied().for_each(|value| {
                rem_acc = rem_acc.add_wrapping(value);
            });

            let mut reduced = T::default_value();
            for v in chunk_acc {
                reduced = reduced.add_wrapping(v);
            }
            let sum = reduced.add_wrapping(rem_acc);

            Some(sum)
        }
        Some(nulls) => {
            // process data in chunks of 64 elements since we also get 64 bits of validity information at a time
            let data_chunks = data.chunks_exact(64);
            let remainder = data_chunks.remainder();

            let bit_chunks = nulls.inner().bit_chunks();
            let remainder_bits = bit_chunks.remainder_bits();

            data_chunks.zip(bit_chunks).for_each(|(chunk, mut mask)| {
                // split chunks further into slices corresponding to the vector length
                // the compiler is able to unroll this inner loop and remove bounds checks
                // since the outer chunk size (64) is always a multiple of the number of lanes
                chunk.chunks_exact(LANES).for_each(|chunk| {
                    let mut chunk: [T::Native; LANES] = chunk.try_into().unwrap();

                    for i in 0..LANES {
                        if mask & (1 << i) == 0 {
                            chunk[i] = T::default_value();
                        }
                        chunk_acc[i] = chunk_acc[i].add_wrapping(chunk[i]);
                    }

                    // skip the shift and avoid overflow for u8 type, which uses 64 lanes.
                    mask >>= LANES % 64;
                })
            });

            remainder.iter().enumerate().for_each(|(i, value)| {
                if remainder_bits & (1 << i) != 0 {
                    rem_acc = rem_acc.add_wrapping(*value);
                }
            });

            let mut reduced = T::default_value();
            for v in chunk_acc {
                reduced = reduced.add_wrapping(v);
            }
            let sum = reduced.add_wrapping(rem_acc);

            Some(sum)
        }
    }
}

macro_rules! bit_operation {
    ($NAME:ident, $OP:ident, $NATIVE:ident, $DEFAULT:expr, $DOC:expr) => {
        #[doc = $DOC]
        ///
        /// Returns `None` if the array is empty or only contains null values.
        pub fn $NAME<T>(array: &PrimitiveArray<T>) -> Option<T::Native>
        where
            T: ArrowNumericType,
            T::Native: $NATIVE<Output = T::Native> + ArrowNativeTypeOp,
        {
            let default;
            if $DEFAULT == -1 {
                default = T::Native::ONE.neg_wrapping();
            } else {
                default = T::default_value();
            }

            let null_count = array.null_count();

            if null_count == array.len() {
                return None;
            }

            let data: &[T::Native] = array.values();

            match array.nulls() {
                None => {
                    let result = data
                        .iter()
                        .fold(default, |accumulator, value| accumulator.$OP(*value));

                    Some(result)
                }
                Some(nulls) => {
                    let mut result = default;
                    let data_chunks = data.chunks_exact(64);
                    let remainder = data_chunks.remainder();

                    let bit_chunks = nulls.inner().bit_chunks();
                    data_chunks
                        .zip(bit_chunks.iter())
                        .for_each(|(chunk, mask)| {
                            // index_mask has value 1 << i in the loop
                            let mut index_mask = 1;
                            chunk.iter().for_each(|value| {
                                if (mask & index_mask) != 0 {
                                    result = result.$OP(*value);
                                }
                                index_mask <<= 1;
                            });
                        });

                    let remainder_bits = bit_chunks.remainder_bits();

                    remainder.iter().enumerate().for_each(|(i, value)| {
                        if remainder_bits & (1 << i) != 0 {
                            result = result.$OP(*value);
                        }
                    });

                    Some(result)
                }
            }
        }
    };
}

bit_operation!(
    bit_and,
    bitand,
    BitAnd,
    -1,
    "Returns the bitwise and of all non-null input values."
);
bit_operation!(
    bit_or,
    bitor,
    BitOr,
    0,
    "Returns the bitwise or of all non-null input values."
);
bit_operation!(
    bit_xor,
    bitxor,
    BitXor,
    0,
    "Returns the bitwise xor of all non-null input values."
);

/// Returns true if all non-null input values are true, otherwise false.
///
/// Returns `None` if the array is empty or only contains null values.
pub fn bool_and(array: &BooleanArray) -> Option<bool> {
    if array.null_count() == array.len() {
        return None;
    }
    Some(array.false_count() == 0)
}

/// Returns true if any non-null input value is true, otherwise false.
///
/// Returns `None` if the array is empty or only contains null values.
pub fn bool_or(array: &BooleanArray) -> Option<bool> {
    if array.null_count() == array.len() {
        return None;
    }
    Some(array.true_count() != 0)
}

/// Returns the sum of values in the primitive array.
///
/// Returns `Ok(None)` if the array is empty or only contains null values.
///
/// This detects overflow and returns an `Err` for that. For an non-overflow-checking variant,
/// use `sum` instead.
pub fn sum_checked<T>(array: &PrimitiveArray<T>) -> Result<Option<T::Native>, ArrowError>
where
    T: ArrowNumericType,
    T::Native: ArrowNativeTypeOp,
{
    let null_count = array.null_count();

    if null_count == array.len() {
        return Ok(None);
    }

    let data: &[T::Native] = array.values();

    match array.nulls() {
        None => {
            let sum = data
                .iter()
                .try_fold(T::default_value(), |accumulator, value| {
                    accumulator.add_checked(*value)
                })?;

            Ok(Some(sum))
        }
        Some(nulls) => {
            let mut sum = T::default_value();

            try_for_each_valid_idx(
                nulls.len(),
                nulls.offset(),
                nulls.null_count(),
                Some(nulls.validity()),
                |idx| {
                    unsafe { sum = sum.add_checked(array.value_unchecked(idx))? };
                    Ok::<_, ArrowError>(())
                },
            )?;

            Ok(Some(sum))
        }
    }
}

#[cfg(feature = "simd")]
mod simd {
    use super::is_nan;
    use arrow_array::*;
    use std::marker::PhantomData;

    pub(super) trait SimdAggregate<T: ArrowNumericType> {
        type ScalarAccumulator;
        type SimdAccumulator;

        /// Returns the accumulator for aggregating scalar values
        fn init_accumulator_scalar() -> Self::ScalarAccumulator;

        /// Returns the accumulator for aggregating simd chunks of values
        fn init_accumulator_chunk() -> Self::SimdAccumulator;

        /// Updates the accumulator with the values of one chunk
        fn accumulate_chunk_non_null(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
        );

        /// Updates the accumulator with the values of one chunk according to the given vector mask
        fn accumulate_chunk_nullable(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
            mask: T::SimdMask,
        );

        /// Updates the accumulator with one value
        fn accumulate_scalar(accumulator: &mut Self::ScalarAccumulator, value: T::Native);

        /// Reduces the vector lanes of the simd accumulator and the scalar accumulator to a single value
        fn reduce(
            simd_accumulator: Self::SimdAccumulator,
            scalar_accumulator: Self::ScalarAccumulator,
        ) -> Option<T::Native>;
    }

    pub(super) struct SumAggregate<T: ArrowNumericType> {
        phantom: PhantomData<T>,
    }

    impl<T: ArrowNumericType> SimdAggregate<T> for SumAggregate<T>
    where
        T::Native: ArrowNativeTypeOp,
    {
        type ScalarAccumulator = T::Native;
        type SimdAccumulator = T::Simd;

        fn init_accumulator_scalar() -> Self::ScalarAccumulator {
            T::default_value()
        }

        fn init_accumulator_chunk() -> Self::SimdAccumulator {
            T::init(Self::init_accumulator_scalar())
        }

        fn accumulate_chunk_non_null(accumulator: &mut T::Simd, chunk: T::Simd) {
            *accumulator = *accumulator + chunk;
        }

        fn accumulate_chunk_nullable(
            accumulator: &mut T::Simd,
            chunk: T::Simd,
            vecmask: T::SimdMask,
        ) {
            let zero = T::init(T::default_value());
            let blended = T::mask_select(vecmask, chunk, zero);

            *accumulator = *accumulator + blended;
        }

        fn accumulate_scalar(accumulator: &mut T::Native, value: T::Native) {
            *accumulator = accumulator.add_wrapping(value)
        }

        fn reduce(
            simd_accumulator: Self::SimdAccumulator,
            scalar_accumulator: Self::ScalarAccumulator,
        ) -> Option<T::Native> {
            // we can't use T::lanes() as the slice len because it is not const,
            // instead always reserve the maximum number of lanes
            let mut tmp = [T::default_value(); 64];
            let slice = &mut tmp[0..T::lanes()];
            T::write(simd_accumulator, slice);

            let mut reduced = Self::init_accumulator_scalar();
            slice
                .iter()
                .for_each(|value| Self::accumulate_scalar(&mut reduced, *value));

            Self::accumulate_scalar(&mut reduced, scalar_accumulator);

            // result can not be None because we checked earlier for the null count
            Some(reduced)
        }
    }

    pub(super) struct MinAggregate<T: ArrowNumericType> {
        phantom: PhantomData<T>,
    }

    impl<T: ArrowNumericType> SimdAggregate<T> for MinAggregate<T>
    where
        T::Native: PartialOrd,
    {
        type ScalarAccumulator = (T::Native, bool);
        type SimdAccumulator = (T::Simd, T::SimdMask);

        fn init_accumulator_scalar() -> Self::ScalarAccumulator {
            (T::default_value(), false)
        }

        fn init_accumulator_chunk() -> Self::SimdAccumulator {
            (T::init(T::default_value()), T::mask_init(false))
        }

        fn accumulate_chunk_non_null(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
        ) {
            let acc_is_nan = !T::eq(accumulator.0, accumulator.0);
            let is_lt = acc_is_nan | T::lt(chunk, accumulator.0);
            let first_or_lt = !accumulator.1 | is_lt;

            accumulator.0 = T::mask_select(first_or_lt, chunk, accumulator.0);
            accumulator.1 = T::mask_init(true);
        }

        fn accumulate_chunk_nullable(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
            vecmask: T::SimdMask,
        ) {
            let acc_is_nan = !T::eq(accumulator.0, accumulator.0);
            let is_lt = vecmask & (acc_is_nan | T::lt(chunk, accumulator.0));
            let first_or_lt = !accumulator.1 | is_lt;

            accumulator.0 = T::mask_select(first_or_lt, chunk, accumulator.0);
            accumulator.1 |= vecmask;
        }

        fn accumulate_scalar(
            accumulator: &mut Self::ScalarAccumulator,
            value: T::Native,
        ) {
            if !accumulator.1 {
                accumulator.0 = value;
            } else {
                let acc_is_nan = is_nan(accumulator.0);
                if acc_is_nan || value < accumulator.0 {
                    accumulator.0 = value
                }
            }
            accumulator.1 = true
        }

        fn reduce(
            simd_accumulator: Self::SimdAccumulator,
            scalar_accumulator: Self::ScalarAccumulator,
        ) -> Option<T::Native> {
            // we can't use T::lanes() as the slice len because it is not const,
            // instead always reserve the maximum number of lanes
            let mut tmp = [T::default_value(); 64];
            let slice = &mut tmp[0..T::lanes()];
            T::write(simd_accumulator.0, slice);

            let mut reduced = Self::init_accumulator_scalar();
            slice
                .iter()
                .enumerate()
                .filter(|(i, _value)| T::mask_get(&simd_accumulator.1, *i))
                .for_each(|(_i, value)| Self::accumulate_scalar(&mut reduced, *value));

            if scalar_accumulator.1 {
                Self::accumulate_scalar(&mut reduced, scalar_accumulator.0);
            }

            if reduced.1 {
                Some(reduced.0)
            } else {
                None
            }
        }
    }

    pub(super) struct MaxAggregate<T: ArrowNumericType> {
        phantom: PhantomData<T>,
    }

    impl<T: ArrowNumericType> SimdAggregate<T> for MaxAggregate<T>
    where
        T::Native: PartialOrd,
    {
        type ScalarAccumulator = (T::Native, bool);
        type SimdAccumulator = (T::Simd, T::SimdMask);

        fn init_accumulator_scalar() -> Self::ScalarAccumulator {
            (T::default_value(), false)
        }

        fn init_accumulator_chunk() -> Self::SimdAccumulator {
            (T::init(T::default_value()), T::mask_init(false))
        }

        fn accumulate_chunk_non_null(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
        ) {
            let chunk_is_nan = !T::eq(chunk, chunk);
            let is_gt = chunk_is_nan | T::gt(chunk, accumulator.0);
            let first_or_gt = !accumulator.1 | is_gt;

            accumulator.0 = T::mask_select(first_or_gt, chunk, accumulator.0);
            accumulator.1 = T::mask_init(true);
        }

        fn accumulate_chunk_nullable(
            accumulator: &mut Self::SimdAccumulator,
            chunk: T::Simd,
            vecmask: T::SimdMask,
        ) {
            let chunk_is_nan = !T::eq(chunk, chunk);
            let is_gt = vecmask & (chunk_is_nan | T::gt(chunk, accumulator.0));
            let first_or_gt = !accumulator.1 | is_gt;

            accumulator.0 = T::mask_select(first_or_gt, chunk, accumulator.0);
            accumulator.1 |= vecmask;
        }

        fn accumulate_scalar(
            accumulator: &mut Self::ScalarAccumulator,
            value: T::Native,
        ) {
            if !accumulator.1 {
                accumulator.0 = value;
            } else {
                let value_is_nan = is_nan(value);
                if value_is_nan || value > accumulator.0 {
                    accumulator.0 = value
                }
            }
            accumulator.1 = true;
        }

        fn reduce(
            simd_accumulator: Self::SimdAccumulator,
            scalar_accumulator: Self::ScalarAccumulator,
        ) -> Option<T::Native> {
            // we can't use T::lanes() as the slice len because it is not const,
            // instead always reserve the maximum number of lanes
            let mut tmp = [T::default_value(); 64];
            let slice = &mut tmp[0..T::lanes()];
            T::write(simd_accumulator.0, slice);

            let mut reduced = Self::init_accumulator_scalar();
            slice
                .iter()
                .enumerate()
                .filter(|(i, _value)| T::mask_get(&simd_accumulator.1, *i))
                .for_each(|(_i, value)| Self::accumulate_scalar(&mut reduced, *value));

            if scalar_accumulator.1 {
                Self::accumulate_scalar(&mut reduced, scalar_accumulator.0);
            }

            if reduced.1 {
                Some(reduced.0)
            } else {
                None
            }
        }
    }

    pub(super) fn simd_aggregation<T: ArrowNumericType, A: SimdAggregate<T>>(
        array: &PrimitiveArray<T>,
    ) -> Option<T::Native> {
        let null_count = array.null_count();

        if null_count == array.len() {
            return None;
        }

        let data: &[T::Native] = array.values();

        let mut chunk_acc = A::init_accumulator_chunk();
        let mut rem_acc = A::init_accumulator_scalar();

        match array.nulls() {
            None => {
                let data_chunks = data.chunks_exact(64);
                let remainder = data_chunks.remainder();

                data_chunks.for_each(|chunk| {
                    chunk.chunks_exact(T::lanes()).for_each(|chunk| {
                        let chunk = T::load(&chunk);
                        A::accumulate_chunk_non_null(&mut chunk_acc, chunk);
                    });
                });

                remainder.iter().for_each(|value| {
                    A::accumulate_scalar(&mut rem_acc, *value);
                });
            }
            Some(nulls) => {
                // process data in chunks of 64 elements since we also get 64 bits of validity information at a time
                let data_chunks = data.chunks_exact(64);
                let remainder = data_chunks.remainder();

                let bit_chunks = nulls.inner().bit_chunks();
                let remainder_bits = bit_chunks.remainder_bits();

                data_chunks.zip(bit_chunks).for_each(|(chunk, mut mask)| {
                    // split chunks further into slices corresponding to the vector length
                    // the compiler is able to unroll this inner loop and remove bounds checks
                    // since the outer chunk size (64) is always a multiple of the number of lanes
                    chunk.chunks_exact(T::lanes()).for_each(|chunk| {
                        let vecmask = T::mask_from_u64(mask);
                        let chunk = T::load(&chunk);

                        A::accumulate_chunk_nullable(&mut chunk_acc, chunk, vecmask);

                        // skip the shift and avoid overflow for u8 type, which uses 64 lanes.
                        mask >>= T::lanes() % 64;
                    });
                });

                remainder.iter().enumerate().for_each(|(i, value)| {
                    if remainder_bits & (1 << i) != 0 {
                        A::accumulate_scalar(&mut rem_acc, *value)
                    }
                });
            }
        }

        A::reduce(chunk_acc, rem_acc)
    }
}

/// Returns the sum of values in the primitive array.
///
/// Returns `None` if the array is empty or only contains null values.
///
/// This doesn't detect overflow in release mode by default. Once overflowing, the result will
/// wrap around. For an overflow-checking variant, use `sum_checked` instead.
#[cfg(feature = "simd")]
pub fn sum<T: ArrowNumericType>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T::Native: ArrowNativeTypeOp,
{
    use simd::*;

    simd::simd_aggregation::<T, SumAggregate<T>>(&array)
}

#[cfg(feature = "simd")]
/// Returns the minimum value in the array, according to the natural order.
/// For floating point arrays any NaN values are considered to be greater than any other non-null value
pub fn min<T: ArrowNumericType>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T::Native: PartialOrd,
{
    use simd::*;

    simd::simd_aggregation::<T, MinAggregate<T>>(&array)
}

#[cfg(feature = "simd")]
/// Returns the maximum value in the array, according to the natural order.
/// For floating point arrays any NaN values are considered to be greater than any other non-null value
pub fn max<T: ArrowNumericType>(array: &PrimitiveArray<T>) -> Option<T::Native>
where
    T::Native: PartialOrd,
{
    use simd::*;

    simd::simd_aggregation::<T, MaxAggregate<T>>(&array)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::types::*;
    use arrow_buffer::NullBuffer;
    use std::sync::Arc;

    #[test]
    fn test_primitive_array_sum() {
        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(15, sum(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_float_sum() {
        let a = Float64Array::from(vec![1.1, 2.2, 3.3, 4.4, 5.5]);
        assert_eq!(16.5, sum(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_sum_with_nulls() {
        let a = Int32Array::from(vec![None, Some(2), Some(3), None, Some(5)]);
        assert_eq!(10, sum(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_sum_all_nulls() {
        let a = Int32Array::from(vec![None, None, None]);
        assert_eq!(None, sum(&a));
    }

    #[test]
    fn test_primitive_array_sum_large_64() {
        // create an array that actually has non-zero values at the invalid indices
        let validity = NullBuffer::new((1..=100).map(|x| x % 3 == 0).collect());
        let c = Int64Array::new((1..=100).collect(), Some(validity));

        assert_eq!(Some((1..=100).filter(|i| i % 3 == 0).sum()), sum(&c));
    }

    #[test]
    fn test_primitive_array_sum_large_32() {
        // create an array that actually has non-zero values at the invalid indices
        let validity = NullBuffer::new((1..=100).map(|x| x % 3 == 0).collect());
        let c = Int32Array::new((1..=100).collect(), Some(validity));
        assert_eq!(Some((1..=100).filter(|i| i % 3 == 0).sum()), sum(&c));
    }

    #[test]
    fn test_primitive_array_sum_large_16() {
        // create an array that actually has non-zero values at the invalid indices
        let validity = NullBuffer::new((1..=100).map(|x| x % 3 == 0).collect());
        let c = Int16Array::new((1..=100).collect(), Some(validity));
        assert_eq!(Some((1..=100).filter(|i| i % 3 == 0).sum()), sum(&c));
    }

    #[test]
    fn test_primitive_array_sum_large_8() {
        // include fewer values than other large tests so the result does not overflow the u8
        // create an array that actually has non-zero values at the invalid indices
        let validity = NullBuffer::new((1..=100).map(|x| x % 33 == 0).collect());
        let c = UInt8Array::new((1..=100).collect(), Some(validity));
        assert_eq!(Some((1..=100).filter(|i| i % 33 == 0).sum()), sum(&c));
    }

    #[test]
    fn test_primitive_array_bit_and() {
        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(0, bit_and(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_and_with_nulls() {
        let a = Int32Array::from(vec![None, Some(2), Some(3), None, None]);
        assert_eq!(2, bit_and(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_and_all_nulls() {
        let a = Int32Array::from(vec![None, None, None]);
        assert_eq!(None, bit_and(&a));
    }

    #[test]
    fn test_primitive_array_bit_or() {
        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(7, bit_or(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_or_with_nulls() {
        let a = Int32Array::from(vec![None, Some(2), Some(3), None, Some(5)]);
        assert_eq!(7, bit_or(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_or_all_nulls() {
        let a = Int32Array::from(vec![None, None, None]);
        assert_eq!(None, bit_or(&a));
    }

    #[test]
    fn test_primitive_array_bit_xor() {
        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(1, bit_xor(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_xor_with_nulls() {
        let a = Int32Array::from(vec![None, Some(2), Some(3), None, Some(5)]);
        assert_eq!(4, bit_xor(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bit_xor_all_nulls() {
        let a = Int32Array::from(vec![None, None, None]);
        assert_eq!(None, bit_xor(&a));
    }

    #[test]
    fn test_primitive_array_bool_and() {
        let a = BooleanArray::from(vec![true, false, true, false, true]);
        assert!(!bool_and(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bool_and_with_nulls() {
        let a = BooleanArray::from(vec![None, Some(true), Some(true), None, Some(true)]);
        assert!(bool_and(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bool_and_all_nulls() {
        let a = BooleanArray::from(vec![None, None, None]);
        assert_eq!(None, bool_and(&a));
    }

    #[test]
    fn test_primitive_array_bool_or() {
        let a = BooleanArray::from(vec![true, false, true, false, true]);
        assert!(bool_or(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bool_or_with_nulls() {
        let a =
            BooleanArray::from(vec![None, Some(false), Some(false), None, Some(false)]);
        assert!(!bool_or(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_bool_or_all_nulls() {
        let a = BooleanArray::from(vec![None, None, None]);
        assert_eq!(None, bool_or(&a));
    }

    #[test]
    fn test_primitive_array_min_max() {
        let a = Int32Array::from(vec![5, 6, 7, 8, 9]);
        assert_eq!(5, min(&a).unwrap());
        assert_eq!(9, max(&a).unwrap());
    }

    #[test]
    fn test_primitive_array_min_max_with_nulls() {
        let a = Int32Array::from(vec![Some(5), None, None, Some(8), Some(9)]);
        assert_eq!(5, min(&a).unwrap());
        assert_eq!(9, max(&a).unwrap());
    }

    #[test]
    fn test_primitive_min_max_1() {
        let a = Int32Array::from(vec![None, None, Some(5), Some(2)]);
        assert_eq!(Some(2), min(&a));
        assert_eq!(Some(5), max(&a));
    }

    #[test]
    fn test_primitive_min_max_float_large_nonnull_array() {
        let a: Float64Array = (0..256).map(|i| Some((i + 1) as f64)).collect();
        // min/max are on boundaries of chunked data
        assert_eq!(Some(1.0), min(&a));
        assert_eq!(Some(256.0), max(&a));

        // max is last value in remainder after chunking
        let a: Float64Array = (0..255).map(|i| Some((i + 1) as f64)).collect();
        assert_eq!(Some(255.0), max(&a));

        // max is first value in remainder after chunking
        let a: Float64Array = (0..257).map(|i| Some((i + 1) as f64)).collect();
        assert_eq!(Some(257.0), max(&a));
    }

    #[test]
    fn test_primitive_min_max_float_large_nullable_array() {
        let a: Float64Array = (0..256)
            .map(|i| {
                if (i + 1) % 3 == 0 {
                    None
                } else {
                    Some((i + 1) as f64)
                }
            })
            .collect();
        // min/max are on boundaries of chunked data
        assert_eq!(Some(1.0), min(&a));
        assert_eq!(Some(256.0), max(&a));

        let a: Float64Array = (0..256)
            .map(|i| {
                if i == 0 || i == 255 {
                    None
                } else {
                    Some((i + 1) as f64)
                }
            })
            .collect();
        // boundaries of chunked data are null
        assert_eq!(Some(2.0), min(&a));
        assert_eq!(Some(255.0), max(&a));

        let a: Float64Array = (0..256)
            .map(|i| if i != 100 { None } else { Some((i) as f64) })
            .collect();
        // a single non-null value somewhere in the middle
        assert_eq!(Some(100.0), min(&a));
        assert_eq!(Some(100.0), max(&a));

        // max is last value in remainder after chunking
        let a: Float64Array = (0..255).map(|i| Some((i + 1) as f64)).collect();
        assert_eq!(Some(255.0), max(&a));

        // max is first value in remainder after chunking
        let a: Float64Array = (0..257).map(|i| Some((i + 1) as f64)).collect();
        assert_eq!(Some(257.0), max(&a));
    }

    #[test]
    fn test_primitive_min_max_float_edge_cases() {
        let a: Float64Array = (0..100).map(|_| Some(f64::NEG_INFINITY)).collect();
        assert_eq!(Some(f64::NEG_INFINITY), min(&a));
        assert_eq!(Some(f64::NEG_INFINITY), max(&a));

        let a: Float64Array = (0..100).map(|_| Some(f64::MIN)).collect();
        assert_eq!(Some(f64::MIN), min(&a));
        assert_eq!(Some(f64::MIN), max(&a));

        let a: Float64Array = (0..100).map(|_| Some(f64::MAX)).collect();
        assert_eq!(Some(f64::MAX), min(&a));
        assert_eq!(Some(f64::MAX), max(&a));

        let a: Float64Array = (0..100).map(|_| Some(f64::INFINITY)).collect();
        assert_eq!(Some(f64::INFINITY), min(&a));
        assert_eq!(Some(f64::INFINITY), max(&a));
    }

    #[test]
    fn test_primitive_min_max_float_all_nans_non_null() {
        let a: Float64Array = (0..100).map(|_| Some(f64::NAN)).collect();
        assert!(max(&a).unwrap().is_nan());
        assert!(min(&a).unwrap().is_nan());
    }

    #[test]
    fn test_primitive_min_max_float_first_nan_nonnull() {
        let a: Float64Array = (0..100)
            .map(|i| {
                if i == 0 {
                    Some(f64::NAN)
                } else {
                    Some(i as f64)
                }
            })
            .collect();
        assert_eq!(Some(1.0), min(&a));
        assert!(max(&a).unwrap().is_nan());
    }

    #[test]
    fn test_primitive_min_max_float_last_nan_nonnull() {
        let a: Float64Array = (0..100)
            .map(|i| {
                if i == 99 {
                    Some(f64::NAN)
                } else {
                    Some((i + 1) as f64)
                }
            })
            .collect();
        assert_eq!(Some(1.0), min(&a));
        assert!(max(&a).unwrap().is_nan());
    }

    #[test]
    fn test_primitive_min_max_float_first_nan_nullable() {
        let a: Float64Array = (0..100)
            .map(|i| {
                if i == 0 {
                    Some(f64::NAN)
                } else if i % 2 == 0 {
                    None
                } else {
                    Some(i as f64)
                }
            })
            .collect();
        assert_eq!(Some(1.0), min(&a));
        assert!(max(&a).unwrap().is_nan());
    }

    #[test]
    fn test_primitive_min_max_float_last_nan_nullable() {
        let a: Float64Array = (0..100)
            .map(|i| {
                if i == 99 {
                    Some(f64::NAN)
                } else if i % 2 == 0 {
                    None
                } else {
                    Some(i as f64)
                }
            })
            .collect();
        assert_eq!(Some(1.0), min(&a));
        assert!(max(&a).unwrap().is_nan());
    }

    #[test]
    fn test_primitive_min_max_float_inf_and_nans() {
        let a: Float64Array = (0..100)
            .map(|i| {
                let x = match i % 10 {
                    0 => f64::NEG_INFINITY,
                    1 => f64::MIN,
                    2 => f64::MAX,
                    4 => f64::INFINITY,
                    5 => f64::NAN,
                    _ => i as f64,
                };
                Some(x)
            })
            .collect();
        assert_eq!(Some(f64::NEG_INFINITY), min(&a));
        assert!(max(&a).unwrap().is_nan());
    }

    #[test]
    fn test_binary_min_max_with_nulls() {
        let a = BinaryArray::from(vec![
            Some("b".as_bytes()),
            None,
            None,
            Some(b"a"),
            Some(b"c"),
        ]);
        assert_eq!(Some("a".as_bytes()), min_binary(&a));
        assert_eq!(Some("c".as_bytes()), max_binary(&a));
    }

    #[test]
    fn test_binary_min_max_no_null() {
        let a = BinaryArray::from(vec![Some("b".as_bytes()), Some(b"a"), Some(b"c")]);
        assert_eq!(Some("a".as_bytes()), min_binary(&a));
        assert_eq!(Some("c".as_bytes()), max_binary(&a));
    }

    #[test]
    fn test_binary_min_max_all_nulls() {
        let a = BinaryArray::from(vec![None, None]);
        assert_eq!(None, min_binary(&a));
        assert_eq!(None, max_binary(&a));
    }

    #[test]
    fn test_binary_min_max_1() {
        let a = BinaryArray::from(vec![None, None, Some("b".as_bytes()), Some(b"a")]);
        assert_eq!(Some("a".as_bytes()), min_binary(&a));
        assert_eq!(Some("b".as_bytes()), max_binary(&a));
    }

    #[test]
    fn test_string_min_max_with_nulls() {
        let a = StringArray::from(vec![Some("b"), None, None, Some("a"), Some("c")]);
        assert_eq!(Some("a"), min_string(&a));
        assert_eq!(Some("c"), max_string(&a));
    }

    #[test]
    fn test_string_min_max_all_nulls() {
        let v: Vec<Option<&str>> = vec![None, None];
        let a = StringArray::from(v);
        assert_eq!(None, min_string(&a));
        assert_eq!(None, max_string(&a));
    }

    #[test]
    fn test_string_min_max_1() {
        let a = StringArray::from(vec![None, None, Some("b"), Some("a")]);
        assert_eq!(Some("a"), min_string(&a));
        assert_eq!(Some("b"), max_string(&a));
    }

    #[test]
    fn test_boolean_min_max_empty() {
        let a = BooleanArray::from(vec![] as Vec<Option<bool>>);
        assert_eq!(None, min_boolean(&a));
        assert_eq!(None, max_boolean(&a));
    }

    #[test]
    fn test_boolean_min_max_all_null() {
        let a = BooleanArray::from(vec![None, None]);
        assert_eq!(None, min_boolean(&a));
        assert_eq!(None, max_boolean(&a));
    }

    #[test]
    fn test_boolean_min_max_no_null() {
        let a = BooleanArray::from(vec![Some(true), Some(false), Some(true)]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));
    }

    #[test]
    fn test_boolean_min_max() {
        let a = BooleanArray::from(vec![Some(true), Some(true), None, Some(false), None]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));

        let a = BooleanArray::from(vec![None, Some(true), None, Some(false), None]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));

        let a =
            BooleanArray::from(vec![Some(false), Some(true), None, Some(false), None]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));
    }

    #[test]
    fn test_boolean_min_max_smaller() {
        let a = BooleanArray::from(vec![Some(false)]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(false), max_boolean(&a));

        let a = BooleanArray::from(vec![None, Some(false)]);
        assert_eq!(Some(false), min_boolean(&a));
        assert_eq!(Some(false), max_boolean(&a));

        let a = BooleanArray::from(vec![None, Some(true)]);
        assert_eq!(Some(true), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));

        let a = BooleanArray::from(vec![Some(true)]);
        assert_eq!(Some(true), min_boolean(&a));
        assert_eq!(Some(true), max_boolean(&a));
    }

    #[test]
    fn test_sum_dyn() {
        let values = Int8Array::from_iter_values([10_i8, 11, 12, 13, 14, 15, 16, 17]);
        let values = Arc::new(values) as ArrayRef;
        let keys = Int8Array::from_iter_values([2_i8, 3, 4]);

        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(39, sum_array::<Int8Type, _>(array).unwrap());

        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(15, sum_array::<Int32Type, _>(&a).unwrap());

        let keys = Int8Array::from(vec![Some(2_i8), None, Some(4)]);
        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(26, sum_array::<Int8Type, _>(array).unwrap());

        let keys = Int8Array::from(vec![None, None, None]);
        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert!(sum_array::<Int8Type, _>(array).is_none());
    }

    #[test]
    fn test_max_min_dyn() {
        let values = Int8Array::from_iter_values([10_i8, 11, 12, 13, 14, 15, 16, 17]);
        let keys = Int8Array::from_iter_values([2_i8, 3, 4]);
        let values = Arc::new(values) as ArrayRef;

        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(14, max_array::<Int8Type, _>(array).unwrap());

        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(12, min_array::<Int8Type, _>(array).unwrap());

        let a = Int32Array::from(vec![1, 2, 3, 4, 5]);
        assert_eq!(5, max_array::<Int32Type, _>(&a).unwrap());
        assert_eq!(1, min_array::<Int32Type, _>(&a).unwrap());

        let keys = Int8Array::from(vec![Some(2_i8), None, Some(7)]);
        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(17, max_array::<Int8Type, _>(array).unwrap());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert_eq!(12, min_array::<Int8Type, _>(array).unwrap());

        let keys = Int8Array::from(vec![None, None, None]);
        let dict_array = DictionaryArray::new(keys, values.clone());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert!(max_array::<Int8Type, _>(array).is_none());
        let array = dict_array.downcast_dict::<Int8Array>().unwrap();
        assert!(min_array::<Int8Type, _>(array).is_none());
    }

    #[test]
    fn test_max_min_dyn_nan() {
        let values = Float32Array::from(vec![5.0_f32, 2.0_f32, f32::NAN]);
        let keys = Int8Array::from_iter_values([0_i8, 1, 2]);

        let dict_array = DictionaryArray::new(keys, Arc::new(values));
        let array = dict_array.downcast_dict::<Float32Array>().unwrap();
        assert!(max_array::<Float32Type, _>(array).unwrap().is_nan());

        let array = dict_array.downcast_dict::<Float32Array>().unwrap();
        assert_eq!(2.0_f32, min_array::<Float32Type, _>(array).unwrap());
    }

    #[test]
    fn test_min_max_sliced_primitive() {
        let expected = Some(4.0);
        let input: Float64Array = vec![None, Some(4.0)].into_iter().collect();
        let actual = min(&input);
        assert_eq!(actual, expected);
        let actual = max(&input);
        assert_eq!(actual, expected);

        let sliced_input: Float64Array = vec![None, None, None, None, None, Some(4.0)]
            .into_iter()
            .collect();
        let sliced_input = sliced_input.slice(4, 2);

        assert_eq!(&sliced_input, &input);

        let actual = min(&sliced_input);
        assert_eq!(actual, expected);
        let actual = max(&sliced_input);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_min_max_sliced_boolean() {
        let expected = Some(true);
        let input: BooleanArray = vec![None, Some(true)].into_iter().collect();
        let actual = min_boolean(&input);
        assert_eq!(actual, expected);
        let actual = max_boolean(&input);
        assert_eq!(actual, expected);

        let sliced_input: BooleanArray = vec![None, None, None, None, None, Some(true)]
            .into_iter()
            .collect();
        let sliced_input = sliced_input.slice(4, 2);

        assert_eq!(sliced_input, input);

        let actual = min_boolean(&sliced_input);
        assert_eq!(actual, expected);
        let actual = max_boolean(&sliced_input);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_min_max_sliced_string() {
        let expected = Some("foo");
        let input: StringArray = vec![None, Some("foo")].into_iter().collect();
        let actual = min_string(&input);
        assert_eq!(actual, expected);
        let actual = max_string(&input);
        assert_eq!(actual, expected);

        let sliced_input: StringArray = vec![None, None, None, None, None, Some("foo")]
            .into_iter()
            .collect();
        let sliced_input = sliced_input.slice(4, 2);

        assert_eq!(&sliced_input, &input);

        let actual = min_string(&sliced_input);
        assert_eq!(actual, expected);
        let actual = max_string(&sliced_input);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_min_max_sliced_binary() {
        let expected: Option<&[u8]> = Some(&[5]);
        let input: BinaryArray = vec![None, Some(&[5])].into_iter().collect();
        let actual = min_binary(&input);
        assert_eq!(actual, expected);
        let actual = max_binary(&input);
        assert_eq!(actual, expected);

        let sliced_input: BinaryArray = vec![None, None, None, None, None, Some(&[5])]
            .into_iter()
            .collect();
        let sliced_input = sliced_input.slice(4, 2);

        assert_eq!(&sliced_input, &input);

        let actual = min_binary(&sliced_input);
        assert_eq!(actual, expected);
        let actual = max_binary(&sliced_input);
        assert_eq!(actual, expected);
    }

    #[test]
    #[cfg(not(feature = "simd"))]
    fn test_sum_overflow() {
        let a = Int32Array::from(vec![i32::MAX, 1]);

        assert_eq!(sum(&a).unwrap(), -2147483648);
        assert_eq!(sum_array::<Int32Type, _>(&a).unwrap(), -2147483648);
    }

    #[test]
    fn test_sum_checked_overflow() {
        let a = Int32Array::from(vec![i32::MAX, 1]);

        sum_checked(&a).expect_err("overflow should be detected");
        sum_array_checked::<Int32Type, _>(&a).expect_err("overflow should be detected");
    }
}
