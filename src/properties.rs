use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    num::NonZeroU64,
    ops::Deref,
    ptr::NonNull,
};

use crate::{sys, Error, JackStr, Result, Uuid};

#[derive(Debug)]
pub struct PropertyKey(pub(crate) *const std::os::raw::c_char);

unsafe impl Send for PropertyKey {}
unsafe impl Sync for PropertyKey {}

impl AsRef<CStr> for PropertyKey {
    fn as_ref(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0) }
    }
}

#[doc(alias = "jack_get_property")]
pub fn property(subject: Uuid, key: impl AsRef<CStr>) -> Result<(CString, Option<CString>)> {
    let mut value = std::ptr::null_mut();
    let mut type_ = std::ptr::null_mut();
    Error::check_ret(unsafe {
        sys::weak_library()?.jack_get_property(
            subject.0.get(),
            key.as_ref().as_ptr(),
            &mut value,
            &mut type_,
        )
    })?;
    let value = unsafe { JackStr::from_raw_unchecked(value) };
    let type_ = NonNull::new(type_).map(JackStr);
    unsafe { Ok((value.as_cstring(), type_.map(|t| t.as_cstring()))) }
}

#[doc(alias = "jack_get_properties")]
pub fn properties(subject: Uuid) -> Result<PropertyDescription> {
    let mut desc = MaybeUninit::uninit();
    Error::check_ret(unsafe {
        sys::weak_library()?.jack_get_properties(subject.0.get(), desc.as_mut_ptr())
    })?;
    Ok(PropertyDescription(unsafe { desc.assume_init() }))
}

#[doc(alias = "jack_property_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct Property(sys::jack_property_t);

unsafe impl Send for Property {}
unsafe impl Sync for Property {}

impl Property {
    pub fn key(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0.key) }
    }
    pub fn data(&self) -> &CStr {
        unsafe { CStr::from_ptr(self.0.data) }
    }
    pub fn type_(&self) -> Option<&CStr> {
        if self.0.type_.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(self.0.type_) })
    }
}

#[doc(alias = "jack_description_t")]
#[repr(transparent)]
#[derive(Debug)]
pub struct PropertyDescription(sys::jack_description_t);

unsafe impl Send for PropertyDescription {}
unsafe impl Sync for PropertyDescription {}

impl PropertyDescription {
    #[inline]
    pub fn subject(&self) -> Uuid {
        Uuid(unsafe { NonZeroU64::new_unchecked(self.0.subject) })
    }
}

impl Deref for PropertyDescription {
    type Target = [Property];

    fn deref(&self) -> &Self::Target {
        unsafe {
            std::slice::from_raw_parts(self.0.properties as *const _, self.0.property_cnt as usize)
        }
    }
}

impl Drop for PropertyDescription {
    fn drop(&mut self) {
        unsafe {
            sys::library().jack_free_description(&mut self.0, 0);
        }
    }
}

#[doc(alias = "jack_get_all_properties")]
pub fn all_properties() -> Result<PropertyDescriptionList> {
    let mut descs = MaybeUninit::uninit();
    let count = unsafe { sys::weak_library()?.jack_get_all_properties(descs.as_mut_ptr()) };
    Error::check_ret(count)?;
    Ok(PropertyDescriptionList {
        ptr: unsafe { descs.assume_init() },
        count: count as u32,
    })
}

#[derive(Debug)]
pub struct PropertyDescriptionList {
    ptr: *mut sys::jack_description_t,
    count: u32,
}

unsafe impl Send for PropertyDescriptionList {}
unsafe impl Sync for PropertyDescriptionList {}

impl Deref for PropertyDescriptionList {
    type Target = [PropertyDescription];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr as *const _, self.count as usize) }
    }
}

impl Drop for PropertyDescriptionList {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                for index in 0..self.count {
                    sys::library().jack_free_description(self.ptr.add(index as usize), 0);
                }
                sys::library().jack_free(self.ptr as *mut _);
            }
        }
    }
}
