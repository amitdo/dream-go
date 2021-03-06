// Copyright 2017 Karl Sundequist Blomdahl <karl.sundequist.blomdahl@gmail.com>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod asm;
mod circular_buf;
mod codegen;
mod small_set;
pub mod sgf;
pub mod symmetry;
mod zobrist;

use std::fmt;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

use self::circular_buf::CircularBuf;
use self::small_set::SmallSet;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Color {
    Black = 1,
    White = 2
}

impl Color {
    /// Returns the opposite of this color.
    pub fn opposite(&self) -> Color {
        match *self {
            Color::Black => Color::White,
            Color::White => Color::Black
        }
    }
}

impl ::std::str::FromStr for Color {
    type Err = ();

    fn from_str(s: &str) -> Result<Color, Self::Err> {
        let s = s.to_lowercase();

        if s == "black" || s == "b" {
            Ok(Color::Black)
        } else if s == "white" || s == "w" {
            Ok(Color::White)
        } else {
            Err(())
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Color::Black => write!(f, "B"),
            Color::White => write!(f, "W")
        }
    }
}

/// Utility function for determining the data format of the array returned by
/// `get_features`.
pub trait Order {
    fn index(c: usize, i: usize) -> usize;
}

/// Implementation of `Order` for the data format `NCHW`.
pub struct CHW;

impl Order for CHW {
    fn index(c: usize, i: usize) -> usize {
        c * 361 + i
    }
}

/// Implementation of `Order` for the data format `NHWC`.
pub struct HWC;

impl Order for HWC {
    fn index(c: usize, i: usize) -> usize {
        i * 32 + c
    }
}

/// Returns `$array[$nested[$index]]` without boundary checks
macro_rules! nested_get_unchecked {
    ($array:expr, $nested:expr, $index:expr) => (unsafe {
        *$array.get_unchecked(*$nested.get_unchecked($index as usize) as usize)
    })
}

macro_rules! N {
    ($array:expr, $index:expr) => (nested_get_unchecked!($array, codegen::N, $index));
    ($index:expr) => (unsafe { *codegen::N.get_unchecked($index as usize) as usize })
}
macro_rules! E {
    ($array:expr, $index:expr) => (nested_get_unchecked!($array, codegen::E, $index));
    ($index:expr) => (unsafe { *codegen::E.get_unchecked($index as usize) as usize })
}
macro_rules! S {
    ($array:expr, $index:expr) => (nested_get_unchecked!($array, codegen::S, $index));
    ($index:expr) => (unsafe { *codegen::S.get_unchecked($index as usize) as usize })
}
macro_rules! W {
    ($array:expr, $index:expr) => (nested_get_unchecked!($array, codegen::W, $index));
    ($index:expr) => (unsafe { *codegen::W.get_unchecked($index as usize) as usize })
}

pub struct Board {
    /// The color of the stone that is occupying each vertex. This array
    /// should in addition contain at least one extra padding element that
    /// contains `0xff`, this extra element is used to the out-of-bounds
    /// index to avoid extra branches.
    vertices: [u8; 368],

    /// The index of a stone that is strongly connected to each vertex in
    /// such a way that every stone in a strongly connected group forms
    /// a cycle.
    next_vertex: [u16; 361],

    /// Stack containing the six most recent `vertices`.
    history: CircularBuf,

    /// The total number of moves that has been played on this board.
    count: u16,

    /// The zobrist hash of the current board state.
    zobrist_hash: u64,

    /// The zobrist hash of the most recent board positions.
    zobrist_history: SmallSet
}

impl Clone for Board {
    fn clone(&self) -> Board {
        Board {
            vertices: self.vertices,
            next_vertex: self.next_vertex,
            history: self.history.clone(),
            count: self.count,
            zobrist_hash: self.zobrist_hash,
            zobrist_history: self.zobrist_history.clone()
        }
    }
}

impl Board {
    /// Returns an empty board state.
    pub fn new() -> Board {
        let mut board = Board {
            vertices: [0; 368],
            next_vertex: [0; 361],
            history: CircularBuf::new(),
            count: 0,
            zobrist_hash: 0,
            zobrist_history: SmallSet::new()
        };

        for i in 361..368 {
            board.vertices[i] = 0xff;
        }

        board
    }

    /// Returns the width and height of this board.
    #[inline]
    pub fn size(&self) -> usize {
        19
    }

    /// Returns the zobrist hash of this board.
    #[inline]
    pub fn zobrist_hash(&self) -> u64 {
        self.zobrist_hash
    }

    /// Returns the number of moves that has been played on this board.
    #[inline]
    pub fn count(&self) -> u16 {
        self.count
    }

    /// Returns the color (if the vertex is not empty) of the stone at
    /// the given coordinates.
    ///
    /// # Arguments
    ///
    /// * `x` - the column of the coordinates
    /// * `y` - the row of the coordinates
    ///
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> Option<Color> {
        let index = 19 * y + x;

        if self.vertices[index] == Color::Black as u8 {
            Some(Color::Black)
        } else if self.vertices[index] == Color::White as u8 {
            Some(Color::White)
        } else {
            None
        }
    }

    /// Returns the index of the first liberty found for the given group.
    ///
    /// # Arguments
    ///
    /// * `vertices` -
    /// * `next_vertex` -
    /// * `index` -
    /// 
    fn get_one_liberty(vertices: &[u8], next_vertex: &[u16], index: usize) -> Option<usize> {
        let mut current = index;

        loop {
            if N!(vertices, current) == 0 { return Some(current + 19); }
            if E!(vertices, current) == 0 { return Some(current + 1); }
            if S!(vertices, current) == 0 { return Some(current - 19); }
            if W!(vertices, current) == 0 { return Some(current - 1); }

            current = next_vertex[current] as usize;
            if current == index {
                break
            }
        }

        None
    }

    /// Returns true iff the group at the given index at least one liberty.
    ///
    /// # Arguments
    ///
    /// * `index` - the index of a stone in the group to check
    ///
    fn has_one_liberty(&self, index: usize) -> bool {
        Board::get_one_liberty(&self.vertices, &self.next_vertex, index).is_some()
    }

    /// Returns true iff the group at the given index has at least two
    /// liberties.
    ///
    /// # Arguments
    ///
    /// * `index` - the index of a stone in the group to check
    ///
    fn has_two_liberties(&self, index: usize) -> bool {
        Board::_has_two_liberties(&self.vertices, &self.next_vertex, index)
    }

    /// Returns true iff the group at the given index has at least two
    /// liberties in the given `vertices` and `next_vertex` arrays.
    ///
    /// # Arguments
    ///
    /// * `vertices` -
    /// * `next_vertex` -
    /// * `index` - the index of a stone in the group to check
    ///
    fn _has_two_liberties(vertices: &[u8], next_vertex: &[u16], index: usize) -> bool {
        let mut current = index;
        let mut previous = 0xffff;

        loop {
            macro_rules! check_two_liberties {
                ($index:expr) => ({
                    if previous != 0xffff && previous != $index {
                        return true;
                    } else {
                        previous = $index;
                    }
                })
            }

            if N!(vertices, current) == 0 { check_two_liberties!(current + 19); }
            if E!(vertices, current) == 0 { check_two_liberties!(current + 1); }
            if S!(vertices, current) == 0 { check_two_liberties!(current - 19); }
            if W!(vertices, current) == 0 { check_two_liberties!(current - 1); }

            current = next_vertex[current] as usize;
            if current == index {
                break
            }
        }

        false
    }

    /// Remove all stones strongly connected to the given index from the board.
    ///
    /// # Arguments
    ///
    /// * `index` - the index of a stone in the group to capture
    ///
    fn capture(&mut self, index: usize) {
        let mut current = index;

        loop {
            let c = self.vertices[current] as usize;

            self.zobrist_hash ^= zobrist::TABLE[c][current];
            self.vertices[current] = 0;

            current = self.next_vertex[current] as usize;
            if current == index {
                break
            }
        }
    }

    /// Remove all stones strongly connected to the given index from the given array
    /// using the group definition from this board.
    ///
    /// # Arguments
    ///
    /// * `index` - the index of a stone in the group to capture
    ///
    fn capture_other(&self, vertices: &mut [u8], index: usize) {
        let mut current = index;

        loop {
            vertices[current] = 0;

            current = self.next_vertex[current] as usize;
            if current == index {
                break
            }
        }
    }

    /// Returns the zobrist hash adjustment that would need to be done if the
    /// group at the given index was capture and was of the given color.
    /// 
    /// # Arguments
    /// 
    /// * `color` - the color of the group to capture
    /// * `index` - the index of a stone in the group
    /// 
    fn capture_if(&self, color: usize, index: usize) -> u64 {
        let mut adjustment = 0;
        let mut current = index;

        loop {
            adjustment ^= zobrist::TABLE[color][current];

            current = self.next_vertex[current] as usize;
            if current == index {
                break
            }
        }

        adjustment
    }

    /// Connects the chains of the two vertices into one chain. This method
    /// should not be called with the same group twice as that will result
    /// in a corrupted chain.
    ///
    /// # Arguments
    ///
    /// * `next_vertex` - the array containing the next vertices
    /// * `index` - the first chain to connect
    /// * `other` - the second chain to connect
    ///
    fn join_vertices(next_vertex: &mut [u16], index: usize, other: usize) {
        // check so that other is not already in the chain starting
        // at index since that would lead to a corrupted chain.
        let mut current = index;

        loop {
            if current == other {
                return;
            }

            current = next_vertex[current] as usize;
            if current == index {
                break;
            }
        }

        // re-connect the two lists so if we have two chains A and B:
        //
        //   A:  a -> b -> c -> a
        //   B:  1 -> 2 -> 3 -> 1
        //
        // then the final new chain will be:
        //
        //   a -> 2 -> 3 -> 1 -> b -> c -> a
        //
        let index_prev = next_vertex[index];
        let other_prev = next_vertex[other];

        next_vertex[other] = index_prev;
        next_vertex[index] = other_prev;
    }

    /// Returns whether the given move is valid according to the
    /// Tromp-Taylor rules.
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the move
    /// * `index` - the HW index of the move
    ///
    pub fn _is_valid(&self, color: Color, index: usize) -> bool {
        self.vertices[index] == 0 && {
            let n = N!(self.vertices, index);
            let e = E!(self.vertices, index);
            let s = S!(self.vertices, index);
            let w = W!(self.vertices, index);

            // check for direct liberties
            if n == 0 { return true; }
            if e == 0 { return true; }
            if s == 0 { return true; }
            if w == 0 { return true; }

            // check for the following two conditions simplied into one case:
            //
            // 1. If a neighbour is friendly then we are fine if it has at
            //    least two liberties.
            // 2. If a neighbour is unfriendly then we are fine if it has less
            //    than two liberties (i.e. one).
            let current = color as u8;

            if n != 0xff && (n == current) == self.has_two_liberties(index + 19) { return true; }
            if e != 0xff && (e == current) == self.has_two_liberties(index + 1) { return true; }
            if s != 0xff && (s == current) == self.has_two_liberties(index - 19) { return true; }
            if w != 0xff && (w == current) == self.has_two_liberties(index - 1) { return true; }

            false  // move is suicide :'(
        }
    }

    /// Returns whether playing the given rule violates the super-ko
    /// rule. This functions assumes the given move is not suicide and
    /// does not play on top of another stone, these pre-conditions can
    /// be checked with the `_is_valid` function.
    /// 
    /// # Arguments
    /// 
    /// * `color` - the color of the move
    /// * `index` - the HW index of the move
    /// 
    pub fn _is_ko(&self, color: Color, index: usize) -> bool {
        let mut zobrist_pretend = self.zobrist_hash ^ zobrist::TABLE[color as usize][index];
        let opponent = color.opposite() as u8;

        if N!(self.vertices, index) == opponent && !self.has_two_liberties(index + 19) {
            zobrist_pretend ^= self.capture_if(opponent as usize, index + 19);
        }
        if E!(self.vertices, index) == opponent && !self.has_two_liberties(index + 1) {
            zobrist_pretend ^= self.capture_if(opponent as usize, index + 1);
        }
        if S!(self.vertices, index) == opponent && !self.has_two_liberties(index - 19) {
            zobrist_pretend ^= self.capture_if(opponent as usize, index - 19);
        }
        if W!(self.vertices, index) == opponent && !self.has_two_liberties(index - 1) {
            zobrist_pretend ^= self.capture_if(opponent as usize, index - 1);
        }

        self.zobrist_history.contains(zobrist_pretend)
    }

    /// Returns whether the given move is valid according to the
    /// Tromp-Taylor rules.
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the move
    /// * `x` - the column of the move
    /// * `y` - the row of the move
    ///
    pub fn is_valid(&self, color: Color, x: usize, y: usize) -> bool {
        let index = 19 * y + x;

        self._is_valid(color, index) && !self._is_ko(color, index)
    }

    /// Place the given stone on the board without checking if it is legal, and
    /// without capturing any of the opponents stones.
    ///
    /// # Arguments
    ///
    /// * `vertices` -
    /// * `next_vertex` -
    /// * `color` - the color of the move
    /// * `index` - the index of the move
    ///
    fn place_no_capture(
        vertices: &mut [u8],
        next_vertex: &mut [u16],
        color: Color,
        index: usize
    ) {
        let player = color as u8;

        // place the stone on the board regardless of whether it is legal
        // or not.
        vertices[index] = color as u8;
        next_vertex[index] = index as u16;

        // connect this stone to any neighbouring groups
        if N!(vertices, index) == player { Board::join_vertices(next_vertex, index, index + 19); }
        if E!(vertices, index) == player { Board::join_vertices(next_vertex, index, index + 1); }
        if S!(vertices, index) == player { Board::join_vertices(next_vertex, index, index - 19); }
        if W!(vertices, index) == player { Board::join_vertices(next_vertex, index, index - 1); }
    }

    /// Place the given stone on the board without checking if it is legal, the
    /// board is then updated according to the Tromp-Taylor rules with the
    /// except that ones own color is not cleared.
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the move
    /// * `x` - The column of the move
    /// * `y` - The row of the move
    ///
    pub fn place(&mut self, color: Color, x: usize, y: usize) {
        let index = 19 * y + x;

        // place the stone on the board regardless of whether it is legal
        // or not.
        Board::place_no_capture(&mut self.vertices, &mut self.next_vertex, color, index);

        self.count += 1;
        self.zobrist_hash ^= zobrist::TABLE[color as usize][index];

        // clear the opponents color
        let opponent = color.opposite() as u8;

        if N!(self.vertices, index) == opponent && !self.has_one_liberty(index + 19) { self.capture(index + 19); }
        if E!(self.vertices, index) == opponent && !self.has_one_liberty(index + 1) { self.capture(index + 1); }
        if S!(self.vertices, index) == opponent && !self.has_one_liberty(index - 19) { self.capture(index - 19); }
        if W!(self.vertices, index) == opponent && !self.has_one_liberty(index - 1) { self.capture(index - 1); }

        // add the current board state to the history *after* we have updated it because:
        //
        // 1. that way we do not need a special case to retrieve the current board when
        //    generating features.
        // 2. the circular stack starts with all buffers as zero, so there is no need to
        //    keep track of the initial board state.
        self.history.push(&self.vertices);
        self.zobrist_history.push(self.zobrist_hash);
    }

    /// Returns true if playing a stone at the given index successfully
    /// captures some stones in a serie of ataris.
    ///
    /// # Arguments
    ///
    /// * `vertices` - the `vertices` of the board to check
    /// * `next_vertex` - the `next_vertex` of the board to check
    /// * `color` - the color of the current player
    /// * `index` - the index of the vertex to check
    ///
    fn _is_ladder_capture(
        vertices: &mut [u8],
        next_vertex: &mut [u16],
        color: Color,
        index: usize
    ) -> bool
    {
        Board::place_no_capture(vertices, next_vertex, color, index);

        // if any of the neighbouring opponent groups were reduced to one
        // liberty then extend into that liberty. if no such group exists
        // then this is not a ladder capturing move.
        let opponent = color.opposite() as u8;
        let opponent_index = {
            macro_rules! check {
                ($dir:ident) => ({
                    if $dir!(vertices, index) == opponent {
                        if Board::_has_two_liberties(vertices, next_vertex, $dir!(index)) {
                            None
                        } else {
                            Board::get_one_liberty(vertices, next_vertex, $dir!(index))
                        }
                    } else {
                        None
                    }
                })
            }

            if let Some(other_index) = check!(N) {
                other_index
            } else if let Some(other_index) = check!(E) {
                other_index
            } else if let Some(other_index) = check!(S) {
                other_index
            } else if let Some(other_index) = check!(W) {
                other_index
            } else {
                return false;
            }
        };

        Board::place_no_capture(vertices, next_vertex, color.opposite(), opponent_index);

        // check the number of liberties after extending the group that was put in atari
        //
        // * If one liberty, then this group can be captured.
        // * If two liberties, keep searching.
        // * If more than two liberties, then this group can not be captured.
        //
        let opponent_count = if N!(vertices, opponent_index) == 0 { 1 } else { 0 }
            + if E!(vertices, opponent_index) == 0 { 1 } else { 0 }
            + if S!(vertices, opponent_index) == 0 { 1 } else { 0 }
            + if W!(vertices, opponent_index) == 0 { 1 } else { 0 };

        if opponent_count < 2 {
            return true;
        } else if opponent_count > 2 {
            return false;
        }

        // if playing `opponent_vertex` put any of my stones into atari
        // then this is not a ladder capturing move.
        let player = color as u8;

        if N!(vertices, opponent_index) == player && !Board::_has_two_liberties(vertices, next_vertex, opponent_index + 19) { return false; }
        if E!(vertices, opponent_index) == player && !Board::_has_two_liberties(vertices, next_vertex, opponent_index + 1) { return false; }
        if S!(vertices, opponent_index) == player && !Board::_has_two_liberties(vertices, next_vertex, opponent_index - 19) { return false; }
        if W!(vertices, opponent_index) == player && !Board::_has_two_liberties(vertices, next_vertex, opponent_index - 1) { return false; }

        // try capturing the new group by playing _ladder capturing moves_
        // in all of its liberties, if we succeed with either then this
        // is a ladder capturing move
        macro_rules! check_recursive {
            ($dir:ident) => ({
                if $dir!(vertices, opponent_index) == 0 {
                    let mut vertices_ = [0; 368];
                    let mut next_vertex_ = [0; 361];

                    vertices_.copy_from_slice(vertices);
                    next_vertex_.copy_from_slice(next_vertex);

                    Board::_is_ladder_capture(&mut vertices_, &mut next_vertex_, color, $dir!(opponent_index))
                } else {
                    false
                }
            })
        }

        if check_recursive!(N) { return true; }
        if check_recursive!(E) { return true; }
        if check_recursive!(S) { return true; }
        if check_recursive!(W) { return true; }

        false
    }

    /// Returns true if playing a stone at the given index allows us to
    /// capture some of the opponents stones with a ladder (sequence of
    /// ataris).
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the current player
    /// * `index` - the index of the stone to check
    ///
    #[allow(unused)]
    fn is_ladder_capture(&self, color: Color, index: usize) -> bool {
        debug_assert!(self._is_valid(color, index));

        // clone only the minimum parts of the board that is necessary
        // to play out the ladder.
        let mut vertices = self.vertices.clone();
        let mut next_vertex = self.next_vertex.clone();

        Board::_is_ladder_capture(&mut vertices, &mut next_vertex, color, index)
    }

    /// Returns true if playing a stone at the given index allows us to
    /// escape using a ladder (sequence of ataris).
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the current player
    /// * `index` - the index of the stone to check
    #[allow(unused)]
    fn is_ladder_escape(&self, color: Color, index: usize) -> bool {
        debug_assert!(self._is_valid(color, index));

        // check if we are connected to a stone with one liberty
        let player = color as u8;
        let connected_to_one = (N!(self.vertices, index) == player && !self.has_two_liberties(index + 19))
            || (E!(self.vertices, index) == player && !self.has_two_liberties(index + 1))
            || (S!(self.vertices, index) == player && !self.has_two_liberties(index - 19))
            || (W!(self.vertices, index) == player && !self.has_two_liberties(index - 1));

        if !connected_to_one {
            return false;
        }

        // clone only the minimum parts of the board that is necessary
        // to play out the ladder.
        let mut vertices = self.vertices.clone();
        let mut next_vertex = self.next_vertex.clone();

        Board::place_no_capture(&mut vertices, &mut next_vertex, color, index);

        // check if we have exactly two liberties
        let liberty_count = if N!(vertices, index) == 0 { 1 } else { 0 }
            + if E!(vertices, index) == 0 { 1 } else { 0 }
            + if S!(vertices, index) == 0 { 1 } else { 0 }
            + if W!(vertices, index) == 0 { 1 } else { 0 };

        if liberty_count != 2 {
            return false;
        }

        // check that we cannot be captured in a ladder from either direction
        macro_rules! check_ladder {
            ($dir:ident) => ({
                let next_index = $dir!(index);

                next_index < 361 && {
                    let mut vertices_ = vertices.clone();
                    let mut next_vertex_ = next_vertex.clone();

                    Board::_is_ladder_capture(&mut vertices_, &mut next_vertex_, color, next_index)
                }
            })
        }

        if check_ladder!(N) { return false; }
        if check_ladder!(E) { return false; }
        if check_ladder!(S) { return false; }
        if check_ladder!(W) { return false; }

        true
    }

    /// Fills the given array with all liberties of in the provided array of vertices
    /// for the group.
    /// 
    /// # Arguments
    /// 
    /// * `vertices` - the array to fill liberties from
    /// * `index` - the group to fill liberties for
    /// * `liberties` - output array containing the liberties of this group
    /// 
    fn fill_liberties(&self, vertices: &[u8], index: usize, liberties: &mut [u8]) {
        let mut current = index;

        loop {
            #![allow(unused_unsafe)]
            unsafe {
                *liberties.get_unchecked_mut(N!(current)) = N!(vertices, current);
                *liberties.get_unchecked_mut(E!(current)) = E!(vertices, current);
                *liberties.get_unchecked_mut(S!(current)) = S!(vertices, current);
                *liberties.get_unchecked_mut(W!(current)) = W!(vertices, current);

                current = *self.next_vertex.get_unchecked(current) as usize;
            }

            if current == index {
                break
            }
        }
    }

    /// Returns the number of liberties of the given group using any recorded
    /// value in `memoize` if available otherwise it is calculated. Any
    /// calculated value is written back to `memoize` for all strongly
    /// connected stones.
    ///
    /// # Arguments
    ///
    /// * `index` - the index of the group to check
    /// * `memoize` - cache of already calculated liberty counts
    ///
    fn get_num_liberties(&self, index: usize, memoize: &mut [usize]) -> usize {
        if memoize[index] != 0 {
            memoize[index]
        } else {
            let mut liberties = [0xff; 384];

            self.fill_liberties(&self.vertices, index, &mut liberties);

            // count the number of liberties, maybe in the future using a SIMD
            // implementation which would be a lot faster than this
            let num_liberties = asm::count_zeros(&liberties);

            // update the cached value in the memoize array for all stones
            // that are strongly connected to the given index
            let mut current = index;

            loop {
                memoize[current] = num_liberties;

                current = self.next_vertex[current] as usize;
                if current == index {
                    break
                }
            }

            num_liberties
        }
    }

    /// Returns whether the given move is valid according to the
    /// Tromp-Taylor rules using the provided `memoize` table to
    /// determine the number of liberties.
    /// 
    /// This function also assume the given vertex is empty and does
    /// not perform the check itself.
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the move
    /// * `index` - the HW index of the move
    /// * `memoize` - cache of already calculated liberty counts
    ///
    fn _is_valid_memoize(&self, color: Color, index: usize, memoize: &mut [usize]) -> bool {
        debug_assert!(self.vertices[index] == 0);

        let n = N!(self.vertices, index);
        let e = E!(self.vertices, index);
        let s = S!(self.vertices, index);
        let w = W!(self.vertices, index);

        // check for direct liberties
        if n == 0 { return true; }
        if e == 0 { return true; }
        if s == 0 { return true; }
        if w == 0 { return true; }

        // check for the following two conditions simplied into one case:
        //
        // 1. If a neighbour is friendly then we are fine if it has at
        //    least two liberties.
        // 2. If a neighbour is unfriendly then we are fine if it has less
        //    than two liberties (i.e. one).
        let current = color as u8;

        if n != 0xff && (n == current) == (self.get_num_liberties(index + 19, memoize) >= 2) { return true; }
        if e != 0xff && (e == current) == (self.get_num_liberties(index + 1, memoize) >= 2) { return true; }
        if s != 0xff && (s == current) == (self.get_num_liberties(index - 19, memoize) >= 2) { return true; }
        if w != 0xff && (w == current) == (self.get_num_liberties(index - 1, memoize) >= 2) { return true; }

        false  // move is suicide :'(
    }

    /// Returns the number of liberties of the group connected to the given stone
    /// *if* it was played, will panic if the vertex is not empty.
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the stone to pretend place
    /// * `index` - the index of the stone to pretend place
    ///
    fn get_num_liberties_if(&self, color: Color, index: usize, memoize: &mut [usize]) -> usize {
        debug_assert!(self.vertices[index] == 0);

        let mut vertices = self.vertices.clone();

        vertices[index] = color as u8;

        // capture of opponent stones 
        let current = color as u8;
        let opponent = color.opposite() as u8;

        if N!(vertices, index) == opponent && self.get_num_liberties(index + 19, memoize) == 1 { self.capture_other(&mut vertices, index + 19); }
        if E!(vertices, index) == opponent && self.get_num_liberties(index + 1, memoize) == 1 { self.capture_other(&mut vertices, index + 1); }
        if S!(vertices, index) == opponent && self.get_num_liberties(index - 19, memoize) == 1 { self.capture_other(&mut vertices, index - 19); }
        if W!(vertices, index) == opponent && self.get_num_liberties(index - 1, memoize) == 1 { self.capture_other(&mut vertices, index - 1); }

        // add liberties based on the liberties of the friendly neighbouring
        // groups
        let mut liberties = [0xff; 384];

        if N!(vertices, index) == current { self.fill_liberties(&vertices, index + 19, &mut liberties); }
        if E!(vertices, index) == current { self.fill_liberties(&vertices, index + 1, &mut liberties); }
        if S!(vertices, index) == current { self.fill_liberties(&vertices, index - 19, &mut liberties); }
        if W!(vertices, index) == current { self.fill_liberties(&vertices, index - 1, &mut liberties); }

        // add direct liberties of the new stone
        liberties[N!(index)] = N!(vertices, index);
        liberties[E!(index)] = E!(vertices, index);
        liberties[S!(index)] = S!(vertices, index);
        liberties[W!(index)] = W!(vertices, index);

        asm::count_zeros(&liberties)
    }

    /// Returns an array containing the (manhattan) distance to the closest stone
    /// of the given color for each point on the board.
    /// 
    /// # Arguments
    /// 
    /// * `color` - the color to get the distance from
    /// 
    fn get_territory_distance(&self, color: Color) -> [u8; 368] {
        let current = color as u8;

        // find all of our stones and mark them as starting points
        let mut territory = [0xff; 368];
        let mut probes = VecDeque::with_capacity(512);

        for index in 0..361 {
            if self.vertices[index] == current {
                territory[index] = 0;
                probes.push_back(index);
            }
        }

        // compute the distance to all neighbours using a dynamic programming
        // approach where we at each iteration try to update the neighbours of
        // each updated vertex, and if the distance we tried to set was smaller
        // than the current distance we try to update that vertex neighbours.
        //
        // This is equivalent to a Bellman–Ford algorithm for the shortest path.
        while !probes.is_empty() {
            let index = probes.pop_front().unwrap();
            let t = territory[index] + 1;

            if N!(self.vertices, index) == 0 && N!(territory, index) > t { probes.push_back(N!(index)); territory[N!(index)] = t; }
            if E!(self.vertices, index) == 0 && E!(territory, index) > t { probes.push_back(E!(index)); territory[E!(index)] = t; }
            if S!(self.vertices, index) == 0 && S!(territory, index) > t { probes.push_back(S!(index)); territory[S!(index)] = t; }
            if W!(self.vertices, index) == 0 && W!(territory, index) > t { probes.push_back(W!(index)); territory[W!(index)] = t; }
        }

        territory
    }

    /// Returns the features of the current board state for the given color,
    /// it returns the following features:
    ///
    /// 1. A constant plane filled with ones
    /// 2. A constant plane filled with ones if we are black
    /// 3. Our liberties (1)
    /// 4. Our liberties (2)
    /// 5. Our liberties (3)
    /// 6. Our liberties (4)
    /// 7. Our liberties (5)
    /// 8. Our liberties (6+)
    /// 9. Our liberties after move (1)
    /// 10. Our liberties after move (2)
    /// 11. Our liberties after move (3)
    /// 12. Our liberties after move (4)
    /// 13. Our liberties after move (5)
    /// 14. Our liberties after move (6+)
    /// 15. Our vertices (now)
    /// 16. Our vertices (now-1)
    /// 17. Our vertices (now-2)
    /// 18. Our vertices (now-3)
    /// 19. Our vertices (now-4)
    /// 20. Our vertices (now-5)
    /// 21. Opponent liberties (1)
    /// 22. Opponent liberties (2)
    /// 23. Opponent liberties (3)
    /// 24. Opponent liberties (4)
    /// 25. Opponent liberties (5)
    /// 26. Opponent liberties (6+)
    /// 27. Opponent vertices (now)
    /// 28. Opponent vertices (now-1)
    /// 29. Opponent vertices (now-2)
    /// 30. Opponent vertices (now-3)
    /// 31. Opponent vertices (now-4)
    /// 32. Opponent vertices (now-5)
    ///
    /// # Arguments
    ///
    /// * `color` - the color of the current player
    ///
    pub fn get_features<T: From<f32> + Copy, O: Order>(
        &self,
        color: Color,
        symmetry: symmetry::Transform
    ) -> Box<[T]>
    {
        let c_0: T = T::from(0.0);
        let c_1: T = T::from(1.0);

        let mut features = vec! [c_0; 32 * 361];
        let symmetry_table = symmetry.get_table();
        let is_black = if color == Color::Black { c_1 } else { c_0 };
        let current = color as u8;

        // set the two constant planes and the liberties
        let mut liberties = [0; 368];

        for index in 0..361 {
            let other = symmetry_table[index] as usize;

            features[O::index(0, other)] = c_1;
            features[O::index(1, other)] = is_black;

            if self.vertices[index] != 0 {
                let num_liberties = ::std::cmp::min(
                    self.get_num_liberties(index, &mut liberties),
                    6
                );
                let l = {
                    debug_assert!(num_liberties > 0);

                    if self.vertices[index] == current {
                        1 + num_liberties
                    } else {
                        19 + num_liberties
                    }
                };

                features[O::index(l, other)] = c_1;
            } else if self._is_valid_memoize(color, index, &mut liberties) {
                let num_liberties = ::std::cmp::min(
                    self.get_num_liberties_if(color, index, &mut liberties),
                    6
                );
                let l = 7 + num_liberties;

                features[O::index(l, other)] = c_1;
            }
        }

        // set the 12 planes that denotes our and the opponents stones
        for (i, vertices) in self.history.iter().enumerate() {
            for index in 0..361 {
                let other = symmetry_table[index] as usize;

                if vertices[index] == 0 {
                    // pass
                } else if vertices[index] == current {
                    let p = 14 + i;

                    features[O::index(p, other)] = c_1;
                } else { // opponent
                    let p = 26 + i;

                    features[O::index(p, other)] = c_1;
                }
            }
        }

        features.into_boxed_slice()
    }

    /// Returns true if this game is fully scorable, a game is
    /// defined as scorable if the following conditions hold:
    /// 
    /// * Both black and white has played at least one stone
    /// * All empty vertices are only reachable from one color
    /// 
    pub fn is_scoreable(&self) -> bool {
        let some_black = (0..361).any(|i| self.vertices[i] == Color::Black as u8);
        let some_white = (0..361).any(|i| self.vertices[i] == Color::White as u8);

        some_black && some_white && {
            let black_distance = self.get_territory_distance(Color::Black);
            let white_distance = self.get_territory_distance(Color::White);

            (0..361).all(|i| black_distance[i] == 0xff || white_distance[i] == 0xff)
        }
    }

    /// Returns the score for each player `(black, white)` of the
    /// current board state according to the Tromp-Taylor rules.
    /// 
    /// This method does not take any komi into account, you will
    /// need to add it yourself.
    pub fn get_score(&self) -> (usize, usize) {
        let mut black = 0;
        let mut white = 0;

        if self.zobrist_hash != 0 {  // at least one stone has been played
            let black_distance = self.get_territory_distance(Color::Black);
            let white_distance = self.get_territory_distance(Color::White);

            for i in 0..361 {
                if black_distance[i] == 0 as u8 {
                    black += 1;  // black has stone at vertex
                } else if white_distance[i] == 0 as u8 {
                    white += 1;  // white has stone at vertex
                } else if white_distance[i] == 0xff {
                    black += 1;  // only reachable from black
                } else if black_distance[i] == 0xff {
                    white += 1;  // only reachable from white
                }
            }
        }

        (black, white)
    }
}

impl fmt::Display for Board {
    /// Pretty-print the current board in a similar format as the KGS client.
    ///
    /// # Arguments
    ///
    /// * `f` - the formatter to write the game to
    ///
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        const LETTERS: [char; 25] = [
            'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'j', 'k',
            'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u',
            'v', 'w', 'x', 'y', 'z'
        ];

        write!(f, "    ")?;
        for i in 0..19 { write!(f, " {}", LETTERS[i])?; }
        write!(f, "\n")?;
        write!(f, "   \u{256d}")?;
        for _ in 0..19 { write!(f, "\u{2500}\u{2500}")?; }
        write!(f, "\u{2500}\u{256e}\n")?;

        for y in 0..19 {
            let y = 18 - y;

            write!(f, "{:2} \u{2502}", 1 + y)?;

            for x in 0..19 {
                let index = 19 * y + x;

                if self.vertices[index] == 0 {
                    write!(f, "  ")?;
                } else if self.vertices[index] == Color::Black as u8 {
                    write!(f, " \u{25cf}")?;
                } else if self.vertices[index] == Color::White as u8 {
                    write!(f, " \u{25cb}")?;
                }
            }

            write!(f, " \u{2502} {}\n", 1 + y)?;
        }

        write!(f, "   \u{2570}")?;
        for _ in 0..19 { write!(f, "\u{2500}\u{2500}")?; }
        write!(f, "\u{2500}\u{256f}\n")?;
        write!(f, "    ")?;
        for i in 0..19 { write!(f, " {}", LETTERS[i])?; }
        write!(f, "\n")?;
        write!(f, "    \u{25cf} Black    \u{25cb} White\n")?;

        Ok(())
    }
}

impl Hash for Board {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // include the entire zobrist hash history, since we use six planes of
        // historic data in the features, and transposing them does not necessary
        // result in the same neural network output (mostly due to super-ko).
        for z in self.zobrist_history.iter() {
            state.write_u64(z);
        }
    }
}

impl PartialEq for Board {
    fn eq(&self, other: &Board) -> bool {
        let history = self.zobrist_history.iter()
            .zip(other.zobrist_history.iter())
            .all(|(a, b)| a == b);

        history && self.vertices.iter().zip(other.vertices.iter()).all(|(a, b)| a == b)
    }
}

impl Eq for Board { }

#[cfg(test)]
mod tests {
    use go::*;

    /// Test that it is possible to capture a stone in the middle of the
    /// board.
    #[test]
    fn capture() {
        let mut board = Board::new();

        board.place(Color::Black,  9,  9);
        board.place(Color::White,  8,  9);
        board.place(Color::White, 10,  9);
        board.place(Color::White,  9,  8);
        board.place(Color::White,  9, 10);

        assert_eq!(board.at(9, 9), None);
    }

    /// Test that it is possible to capture a group of stones in the corner.
    #[test]
    fn capture_group() {
        let mut board = Board::new();

        board.place(Color::Black, 0, 1);
        board.place(Color::Black, 1, 0);
        board.place(Color::Black, 0, 0);
        board.place(Color::Black, 1, 1);

        board.place(Color::White, 2, 0);
        board.place(Color::White, 2, 1);
        board.place(Color::White, 0, 2);
        board.place(Color::White, 1, 2);

        assert_eq!(board.at(0, 0), None);
        assert_eq!(board.at(0, 1), None);
        assert_eq!(board.at(1, 0), None);
        assert_eq!(board.at(1, 1), None);
    }

    /// Test that it is not possible to play a suicide move in the corner
    /// with two adjacent neighbours of the opposite color.
    #[test]
    fn suicide_corner() {
        let mut board = Board::new();

        board.place(Color::White, 0, 0);
        board.place(Color::Black, 1, 0);
        board.place(Color::Black, 0, 1);

        assert_eq!(board.at(0, 0), None);
        assert!(!board.is_valid(Color::White, 0, 0));
        assert!(board.is_valid(Color::Black, 0, 0));
    }

    /// Test that it is not possible to play a suicide move in the middle
    /// of a ponnuki.
    #[test]
    fn suicide_middle() {
        let mut board = Board::new();

        board.place(Color::Black,  9,  9);
        board.place(Color::White,  8,  9);
        board.place(Color::White, 10,  9);
        board.place(Color::White,  9,  8);
        board.place(Color::White,  9, 10);

        assert_eq!(board.at(9, 9), None);
        assert!(!board.is_valid(Color::Black, 9, 9));
        assert!(board.is_valid(Color::White, 9, 9));
    }

    /// Test so that the correct number of pretend liberties are correct.
    #[test]
    fn liberties_if() {
        let mut liberties = [0; 368];
        let mut board = Board::new();

        board.place(Color::White, 0, 0);
        board.place(Color::Black, 0, 1);
        board.place(Color::Black, 1, 1);

        assert_eq!(board.get_num_liberties_if(Color::Black, 1, &mut liberties), 5);
    }

    /// Test that we can accurately detect ko using the simplest possible
    /// corner ko.
    #[test]
    fn ko() {
        let mut board = Board::new();

        board.place(Color::Black, 0, 0);
        board.place(Color::Black, 0, 2);
        board.place(Color::Black, 1, 1);
        board.place(Color::White, 1, 0);
        board.place(Color::White, 0, 1);

        assert!(!board.is_valid(Color::Black, 0, 0));
    }

    #[test]
    fn score_black() {
        let mut board = Board::new();
        board.place(Color::Black, 0, 0);

        assert!(!board.is_scoreable());
        assert_eq!(board.get_score(), (361, 0));
    }

    #[test]
    fn score_white() {
        let mut board = Board::new();
        board.place(Color::White, 0, 0);

        assert!(!board.is_scoreable());
        assert_eq!(board.get_score(), (0, 361));
    }

    #[test]
    fn score_black_white() {
        let mut board = Board::new();
        board.place(Color::White, 1, 0);
        board.place(Color::White, 0, 1);
        board.place(Color::White, 1, 1);
        board.place(Color::Black, 2, 0);
        board.place(Color::Black, 2, 1);
        board.place(Color::Black, 0, 2);
        board.place(Color::Black, 1, 2);

        assert!(board.is_scoreable());
        assert_eq!(board.get_score(), (357, 4));
    }

    #[test]
    fn ladder_corner_capture() {
        // test the following (as 19x19 board), and check
        // that any atari move is a ladder capture
        //
        // X . . . X
        // . . . . .
        // . . . . .
        // . . . . .
        // X . . . X
        //
        let mut board = Board::new();
        board.place(Color::Black,  0,  0);
        board.place(Color::Black,  0, 18);
        board.place(Color::Black, 18,  0);
        board.place(Color::Black, 18, 18);

        for x in 0..19 {
            for y in 0..19 {
                if board.is_valid(Color::White, x, y) {
                    let is_ladder = (x == 1 && y == 0)
                        || (x ==  0 && y ==  1)
                        || (x == 18 && y == 17)
                        || (x == 17 && y == 18)
                        || (x ==  1 && y == 18)
                        || (x == 18 && y ==  1)
                        || (x ==  0 && y == 17)
                        || (x == 17 && y ==  0);
                    let index = 19 * y + x;

                    assert_eq!(board.is_ladder_capture(Color::White, index), is_ladder);
                }
            }
        }
    }

    #[test]
    fn ladder_capture() {
        // test that the most standard ladder capture is detected correctly:
        //
        // . . . . .
        // . . X X .
        // . X O . .
        // . . . . .
        // . . . . .
        //
        let mut board = Board::new();
        board.place(Color::White, 3, 3);
        board.place(Color::Black, 2, 3);
        board.place(Color::Black, 3, 2);
        board.place(Color::Black, 4, 2);

        for x in 0..19 {
            for y in 0..19 {
                if board.is_valid(Color::Black, x, y) {
                    let is_ladder = x == 3 && y == 4;
                    let index = 19 * y + x;

                    assert_eq!(board.is_ladder_capture(Color::Black, index), is_ladder);
                }
            }
        }
    }

    #[test]
    fn ladder_escape() {
        // test a standard ladder pattern with a stone on the diagonal
        let mut board = Board::new();
        board.place(Color::White, 3, 3);
        board.place(Color::White, 15, 15);
        board.place(Color::Black, 2, 3);
        board.place(Color::Black, 3, 2);
        board.place(Color::Black, 4, 2);
        board.place(Color::Black, 3, 4);

        for x in 0..19 {
            for y in 0..19 {
                if board.is_valid(Color::White, x, y) {
                    let is_escape = x == 4 && y == 3;
                    let index = 19 * y + x;

                    assert!(!board.is_ladder_capture(Color::Black, index));
                    assert_eq!(board.is_ladder_escape(Color::White, index), is_escape, "({}, {}) is a ladder escape = {}", x, y, is_escape);
                }
            }
        }
    }
}
