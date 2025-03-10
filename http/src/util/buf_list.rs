//! Copied from `hyper::common::buf`. A vectored buffer.

use std::{collections::VecDeque, io::IoSlice};

use crate::bytes::{Buf, BufMut, Bytes, BytesMut};

pub struct BufList<B> {
    bufs: VecDeque<B>,
    remaining: usize,
}

impl<B: Buf> Default for BufList<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: Buf> BufList<B> {
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bufs: VecDeque::with_capacity(cap),
            remaining: 0,
        }
    }

    #[inline]
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    #[inline]
    pub fn push(&mut self, buf: B) {
        debug_assert!(buf.has_remaining());
        self.remaining += buf.remaining();
        self.bufs.push_back(buf);
    }

    #[inline]
    pub fn cnt(&self) -> usize {
        self.bufs.len()
    }
}

impl<B: Buf> Buf for BufList<B> {
    #[inline]
    fn remaining(&self) -> usize {
        self.remaining
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        self.bufs.front().map(Buf::chunk).unwrap_or_default()
    }

    #[inline]
    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        assert!(!dst.is_empty());
        let mut vecs = 0;
        for buf in &self.bufs {
            vecs += buf.chunks_vectored(&mut dst[vecs..]);
            if vecs == dst.len() {
                break;
            }
        }
        vecs
    }

    #[inline]
    fn advance(&mut self, mut cnt: usize) {
        debug_assert!(self.remaining >= cnt);
        self.remaining -= cnt;
        while cnt > 0 {
            {
                let front = &mut self.bufs[0];
                let rem = front.remaining();
                if rem > cnt {
                    front.advance(cnt);
                    return;
                } else {
                    front.advance(rem);
                    cnt -= rem;
                }
            }
            self.bufs.pop_front();
        }
    }

    #[inline]
    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        // Our inner buffer may have an optimized version of copy_to_bytes, and if the whole
        // request can be fulfilled by the front buffer, we can take advantage.
        match self.bufs.front_mut() {
            Some(front) if front.remaining() == len => {
                let b = front.copy_to_bytes(len);
                self.remaining -= len;
                self.bufs.pop_front();
                b
            }
            Some(front) if front.remaining() > len => {
                self.remaining -= len;
                front.copy_to_bytes(len)
            }
            _ => {
                assert!(len <= self.remaining(), "`len` greater than remaining");
                let mut bm = BytesMut::with_capacity(len);
                bm.put(self.take(len));
                bm.freeze()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    fn hello_world_buf() -> BufList<Bytes> {
        let bufs = vec![Bytes::from("Hello"), Bytes::from(" "), Bytes::from("World")];
        let remaining = bufs.iter().map(Buf::remaining).sum();
        BufList {
            bufs: bufs.into(),
            remaining,
        }
    }

    #[test]
    fn to_bytes_shorter() {
        let mut bufs = hello_world_buf();
        let old_ptr = bufs.chunk().as_ptr();
        let start = bufs.copy_to_bytes(4);
        assert_eq!(start, "Hell");
        assert!(ptr::eq(old_ptr, start.as_ptr()));
        assert_eq!(bufs.chunk(), b"o");
        assert!(ptr::eq(old_ptr.wrapping_add(4), bufs.chunk().as_ptr()));
        assert_eq!(bufs.remaining(), 7);
    }

    #[test]
    fn to_bytes_eq() {
        let mut bufs = hello_world_buf();
        let old_ptr = bufs.chunk().as_ptr();
        let start = bufs.copy_to_bytes(5);
        assert_eq!(start, "Hello");
        assert!(ptr::eq(old_ptr, start.as_ptr()));
        assert_eq!(bufs.chunk(), b" ");
        assert_eq!(bufs.remaining(), 6);
    }

    #[test]
    fn to_bytes_longer() {
        let mut bufs = hello_world_buf();
        let start = bufs.copy_to_bytes(7);
        assert_eq!(start, "Hello W");
        assert_eq!(bufs.remaining(), 4);
    }

    #[test]
    fn one_long_buf_to_bytes() {
        let mut buf = BufList::new();
        buf.push(b"Hello World" as &[_]);
        assert_eq!(buf.copy_to_bytes(5), "Hello");
        assert_eq!(buf.chunk(), b" World");
    }

    #[test]
    #[should_panic(expected = "`len` greater than remaining")]
    fn buf_to_bytes_too_many() {
        hello_world_buf().copy_to_bytes(42);
    }
}
