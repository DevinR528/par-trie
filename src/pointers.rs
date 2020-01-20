//! this is `crossbeam::epoch::atomic` with added debug implementations
//! 
//! All code is from the crossbeam crate except `impl Debug for *`
//! 
//! 
//! The MIT License (MIT)
//! 
//! Copyright (c) 2019 The Crossbeam Project Developers
//! 
//! Permission is hereby granted, free of charge, to any
//! person obtaining a copy of this software and associated
//! documentation files (the "Software"), to deal in the
//! Software without restriction, including without
//! limitation the rights to use, copy, modify, merge,
//! publish, distribute, sublicense, and/or sell copies of
//! the Software, and to permit persons to whom the Software
//! is furnished to do so, subject to the following
//! conditions:
//! 
//! The above copyright notice and this permission notice
//! shall be included in all copies or substantial portions
//! of the Software.
//! 
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
//! ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
//! TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
//! PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
//! SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
//! CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
//! OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
//! IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
//! DEALINGS IN THE SOFTWARE.

use std::borrow::{Borrow, BorrowMut};
use std::cmp;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_utils::atomic::AtomicConsume;
use crossbeam_epoch::{self as epoch, Guard};

#[inline]
fn strongest_failure_ordering(ord: Ordering) -> Ordering {
    use self::Ordering::*;
    match ord {
        Relaxed | Release => Relaxed,
        Acquire | AcqRel => Acquire,
        _ => SeqCst,
    }
}

pub struct CompareAndSetError<'g, T: 'g, P: Pointer<T>> {
    pub current: Shared<'g, T>,

    pub new: P,
}

impl<'g, T: 'g + fmt::Debug, P: Pointer<T> + fmt::Debug> fmt::Debug for CompareAndSetError<'g, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CompareAndSetError")
            .field("current", &self.current)
            .field("new", &self.new)
            .finish()
    }
}

pub trait CompareAndSetOrdering {
    fn success(&self) -> Ordering;

    fn failure(&self) -> Ordering;
}

impl CompareAndSetOrdering for Ordering {
    #[inline]
    fn success(&self) -> Ordering {
        *self
    }

    #[inline]
    fn failure(&self) -> Ordering {
        strongest_failure_ordering(*self)
    }
}

impl CompareAndSetOrdering for (Ordering, Ordering) {
    #[inline]
    fn success(&self) -> Ordering {
        self.0
    }

    #[inline]
    fn failure(&self) -> Ordering {
        self.1
    }
}

#[inline]
fn ensure_aligned<T>(raw: *const T) {
    assert_eq!(raw as usize & low_bits::<T>(), 0, "unaligned pointer");
}

#[inline]
fn low_bits<T>() -> usize {
    (1 << mem::align_of::<T>().trailing_zeros()) - 1
}

#[inline]
fn data_with_tag<T>(data: usize, tag: usize) -> usize {
    (data & !low_bits::<T>()) | (tag & low_bits::<T>())
}

#[inline]
fn decompose_data<T>(data: usize) -> (*mut T, usize) {
    let raw = (data & !low_bits::<T>()) as *mut T;
    let tag = data & low_bits::<T>();
    (raw, tag)
}

pub struct Atomic<T> {
    data: AtomicUsize,
    _marker: PhantomData<*mut T>,
}

unsafe impl<T: Send + Sync> Send for Atomic<T> {}
unsafe impl<T: Send + Sync> Sync for Atomic<T> {}

impl<T> Atomic<T> {
    fn from_usize(data: usize) -> Self {
        Self {
            data: AtomicUsize::new(data),
            _marker: PhantomData,
        }
    }

    #[cfg(not(has_min_const_fn))]
    pub fn null() -> Atomic<T> {
        Self {
            data: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    #[cfg(has_min_const_fn)]
    pub const fn null() -> Atomic<T> {
        Self {
            data: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    pub fn new(value: T) -> Atomic<T> {
        Self::from(Owned::new(value))
    }

    pub fn load<'g>(&self, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.load(ord)) }
    }

    pub fn load_consume<'g>(&self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.load_consume()) }
    }

    pub fn store<'g, P: Pointer<T>>(&self, new: P, ord: Ordering) {
        self.data.store(new.into_usize(), ord);
    }

    pub fn swap<'g, P: Pointer<T>>(&self, new: P, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.swap(new.into_usize(), ord)) }
    }

    pub fn compare_and_set<'g, O, P>(
        &self,
        current: Shared<T>,
        new: P,
        ord: O,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareAndSetError<'g, T, P>>
    where
        O: CompareAndSetOrdering,
        P: Pointer<T>,
    {
        let new = new.into_usize();
        self.data
            .compare_exchange(current.into_usize(), new, ord.success(), ord.failure())
            .map(|_| unsafe { Shared::from_usize(new) })
            .map_err(|current| unsafe {
                CompareAndSetError {
                    current: Shared::from_usize(current),
                    new: P::from_usize(new),
                }
            })
    }

    pub fn compare_and_set_weak<'g, O, P>(
        &self,
        current: Shared<T>,
        new: P,
        ord: O,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareAndSetError<'g, T, P>>
    where
        O: CompareAndSetOrdering,
        P: Pointer<T>,
    {
        let new = new.into_usize();
        self.data
            .compare_exchange_weak(current.into_usize(), new, ord.success(), ord.failure())
            .map(|_| unsafe { Shared::from_usize(new) })
            .map_err(|current| unsafe {
                CompareAndSetError {
                    current: Shared::from_usize(current),
                    new: P::from_usize(new),
                }
            })
    }

    pub fn fetch_and<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_and(val | !low_bits::<T>(), ord)) }
    }

    pub fn fetch_or<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_or(val & low_bits::<T>(), ord)) }
    }

    pub fn fetch_xor<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_xor(val & low_bits::<T>(), ord)) }
    }

    pub unsafe fn into_owned(self) -> Owned<T> {
        Owned::from_usize(self.data.into_inner())
    }
}

impl<T: fmt::Debug> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let g = epoch::pin();
        let data = self.data.load(Ordering::SeqCst);
        let (raw, tag) = decompose_data::<T>(data);

        let shared = self.load(Ordering::SeqCst, &g);
        let inner = if shared.is_null() {
            &"null" as &dyn fmt::Debug
        } else {
            unsafe { shared.deref() as &dyn fmt::Debug }
        };
        f.debug_struct("Atomic")
            .field("raw", &raw)
            .field("inner", inner)
            .field("tag", &tag)
            .finish()
    }
}

impl<T> fmt::Pointer for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data.load(Ordering::SeqCst);
        let (raw, _) = decompose_data::<T>(data);
        fmt::Pointer::fmt(&raw, f)
    }
}

impl<T> Clone for Atomic<T> {
    fn clone(&self) -> Self {
        let data = self.data.load(Ordering::Relaxed);
        Atomic::from_usize(data)
    }
}

impl<T> Default for Atomic<T> {
    fn default() -> Self {
        Atomic::null()
    }
}

impl<T> From<Owned<T>> for Atomic<T> {
    fn from(owned: Owned<T>) -> Self {
        let data = owned.data;
        mem::forget(owned);
        Self::from_usize(data)
    }
}

impl<T> From<Box<T>> for Atomic<T> {
    fn from(b: Box<T>) -> Self {
        Self::from(Owned::from(b))
    }
}

impl<T> From<T> for Atomic<T> {
    fn from(t: T) -> Self {
        Self::new(t)
    }
}

impl<'g, T> From<Shared<'g, T>> for Atomic<T> {
    fn from(ptr: Shared<'g, T>) -> Self {
        Self::from_usize(ptr.data)
    }
}

impl<T> From<*const T> for Atomic<T> {
    fn from(raw: *const T) -> Self {
        Self::from_usize(raw as usize)
    }
}

pub trait Pointer<T> {
    fn into_usize(self) -> usize;

    unsafe fn from_usize(data: usize) -> Self;
}

pub struct Owned<T> {
    data: usize,
    _marker: PhantomData<Box<T>>,
}

impl<T> Pointer<T> for Owned<T> {
    #[inline]
    fn into_usize(self) -> usize {
        let data = self.data;
        mem::forget(self);
        data
    }

    #[inline]
    unsafe fn from_usize(data: usize) -> Self {
        debug_assert!(data != 0, "converting zero into `Owned`");
        Owned {
            data: data,
            _marker: PhantomData,
        }
    }
}

impl<T> Owned<T> {
    pub fn new(value: T) -> Owned<T> {
        Self::from(Box::new(value))
    }

    pub unsafe fn from_raw(raw: *mut T) -> Owned<T> {
        ensure_aligned(raw);
        Self::from_usize(raw as usize)
    }

    pub fn into_shared<'g>(self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.into_usize()) }
    }

    pub fn into_box(self) -> Box<T> {
        let (raw, _) = decompose_data::<T>(self.data);
        mem::forget(self);
        unsafe { Box::from_raw(raw) }
    }

    pub fn tag(&self) -> usize {
        let (_, tag) = decompose_data::<T>(self.data);
        tag
    }

    pub fn with_tag(self, tag: usize) -> Owned<T> {
        let data = self.into_usize();
        unsafe { Self::from_usize(data_with_tag::<T>(data, tag)) }
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe {
            drop(Box::from_raw(raw));
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Owned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data;
        let (raw, tag) = decompose_data::<T>(data);

        let inner = unsafe { self.deref() as &dyn fmt::Debug };
        f.debug_struct("Atomic")
            .field("raw", &raw)
            .field("inner", inner)
            .field("tag", &tag)
            .finish()
    }
}

impl<T: Clone> Clone for Owned<T> {
    fn clone(&self) -> Self {
        Owned::new((**self).clone()).with_tag(self.tag())
    }
}

impl<T> Deref for Owned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe { &*raw }
    }
}

impl<T> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe { &mut *raw }
    }
}

impl<T> From<T> for Owned<T> {
    fn from(t: T) -> Self {
        Owned::new(t)
    }
}

impl<T> From<Box<T>> for Owned<T> {
    fn from(b: Box<T>) -> Self {
        unsafe { Self::from_raw(Box::into_raw(b)) }
    }
}

impl<T> Borrow<T> for Owned<T> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T> BorrowMut<T> for Owned<T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T> AsRef<T> for Owned<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T> AsMut<T> for Owned<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

pub struct Shared<'g, T: 'g> {
    data: usize,
    _marker: PhantomData<(&'g (), *const T)>,
}

impl<'g, T> Clone for Shared<'g, T> {
    fn clone(&self) -> Self {
        Shared {
            data: self.data,
            _marker: PhantomData,
        }
    }
}

impl<'g, T> Copy for Shared<'g, T> {}

impl<'g, T> Pointer<T> for Shared<'g, T> {
    #[inline]
    fn into_usize(self) -> usize {
        self.data
    }

    #[inline]
    unsafe fn from_usize(data: usize) -> Self {
        Shared {
            data: data,
            _marker: PhantomData,
        }
    }
}

impl<'g, T> Shared<'g, T> {
    pub fn null() -> Shared<'g, T> {
        Shared {
            data: 0,
            _marker: PhantomData,
        }
    }

    pub fn is_null(&self) -> bool {
        self.as_raw().is_null()
    }

    pub fn as_raw(&self) -> *const T {
        let (raw, _) = decompose_data::<T>(self.data);
        raw
    }

    pub unsafe fn deref(&self) -> &'g T {
        &*self.as_raw()
    }

    pub unsafe fn deref_mut(&mut self) -> &'g mut T {
        &mut *(self.as_raw() as *mut T)
    }

    pub unsafe fn as_ref(&self) -> Option<&'g T> {
        self.as_raw().as_ref()
    }

    pub unsafe fn into_owned(self) -> Owned<T> {
        debug_assert!(
            self.as_raw() != ptr::null(),
            "converting a null `Shared` into `Owned`"
        );
        Owned::from_usize(self.data)
    }

    pub fn tag(&self) -> usize {
        let (_, tag) = decompose_data::<T>(self.data);
        tag
    }

    pub fn with_tag(&self, tag: usize) -> Shared<'g, T> {
        unsafe { Self::from_usize(data_with_tag::<T>(self.data, tag)) }
    }
}

impl<'g, T> From<*const T> for Shared<'g, T> {
    fn from(raw: *const T) -> Self {
        ensure_aligned(raw);
        unsafe { Self::from_usize(raw as usize) }
    }
}

impl<'g, T> PartialEq<Shared<'g, T>> for Shared<'g, T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<'g, T> Eq for Shared<'g, T> {}

impl<'g, T> PartialOrd<Shared<'g, T>> for Shared<'g, T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.data.partial_cmp(&other.data)
    }
}

impl<'g, T> Ord for Shared<'g, T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.data.cmp(&other.data)
    }
}

impl<'g, T: fmt::Debug> fmt::Debug for Shared<'g, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data;
        let (raw, tag) = decompose_data::<T>(data);

        let inner = if self.is_null() {
            &"null" as &dyn fmt::Debug
        } else {
            unsafe { self.deref() as &dyn fmt::Debug }
        };
        f.debug_struct("Atomic")
            .field("raw", &raw)
            .field("inner", inner)
            .field("tag", &tag)
            .finish()
    }
}

impl<'g, T> fmt::Pointer for Shared<'g, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_raw(), f)
    }
}

impl<'g, T> Default for Shared<'g, T> {
    fn default() -> Self {
        Shared::null()
    }
}

#[cfg(test)]
mod tests {
    use super::Shared;

    #[test]
    fn valid_tag_i8() {
        Shared::<i8>::null().with_tag(0);
    }

    #[test]
    fn valid_tag_i64() {
        Shared::<i64>::null().with_tag(7);
    }
}
