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

/// 
pub struct CircularIterator<'a> {
    count: usize,
    position: usize,
    buf: &'a [[u8; 368]]
}

/// Lookup table computing `(index + 1) % 6`.
const N_MOD_SIX: [usize; 6] = [1, 2, 3, 4, 5, 0];

/// Lookup table computing `(index - 1) % 6` with wrap-around for negative
/// numbers.
const P_MOD_SIX: [usize; 6] = [5, 0, 1, 2, 3, 4];

impl<'a> Iterator for CircularIterator<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.count == 6 {
            None
        } else {
            let index = self.position;
            self.position = P_MOD_SIX[self.position];
            self.count += 1;

            Some(&self.buf[index])
        }
    }
}

/// A circular stack that keeps track of the six most recent pushed buffers.
pub struct CircularBuf {
    position: usize,
    buf: [[u8; 368]; 6]
}

impl Clone for CircularBuf {
    fn clone(&self) -> CircularBuf {
        CircularBuf {
            position: self.position,
            buf: self.buf
        }
    }
}

impl CircularBuf {
    pub fn new() -> CircularBuf {
        CircularBuf {
            position: 0,
            buf: [[0; 368]; 6]
        }
    }

    /// Adds another buffer to this stack.
    /// 
    /// # Arguments
    /// 
    /// * `buf` - 
    /// 
    pub fn push(&mut self, buf: &[u8]) {
        self.buf[self.position].copy_from_slice(buf);
        self.position = N_MOD_SIX[self.position];
    }

    /// Returns an iterator over all the buffers in the stack starting with the
    /// most recent one, and going backward in time.
    pub fn iter<'a>(&'a self) -> CircularIterator<'a> {
        CircularIterator {
            count: 0,
            position: P_MOD_SIX[self.position],
            buf: &self.buf
        }
    }
}

#[cfg(test)]
mod tests {
    use go::circular_buf::*;

    #[test]
    fn check() {
        let mut buf = CircularBuf::new();

        buf.push(&[0; 368]);
        buf.push(&[1; 368]);
        buf.push(&[2; 368]);
        buf.push(&[3; 368]);
        buf.push(&[4; 368]);
        buf.push(&[5; 368]);
        buf.push(&[6; 368]);
        buf.push(&[7; 368]);
        buf.push(&[8; 368]);

        let mut iter = buf.iter();

        assert_eq!(iter.next().unwrap()[0], 8);
        assert_eq!(iter.next().unwrap()[0], 7);
        assert_eq!(iter.next().unwrap()[0], 6);
        assert_eq!(iter.next().unwrap()[0], 5);
        assert_eq!(iter.next().unwrap()[0], 4);
        assert_eq!(iter.next().unwrap()[0], 3);
        assert!(iter.next().is_none());
    }
}