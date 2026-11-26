use std::cell::Cell;

use windows::Win32::{
    Foundation::{E_INVALIDARG, E_POINTER, S_FALSE},
    UI::TextServices::{
        IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
        ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_DISPLAYATTRIBUTE, TF_LS_SOLID,
    },
};
use windows_core::{BSTR, GUID, Result, implement};

use crate::ModuleRef;

pub(crate) const PREEDIT_ATTRIBUTE: GUID = GUID::from_u128(0x28c729bd_6d14_4374_b262_68a4f457fcaa);

fn default_attribute() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        lsStyle: TF_LS_SOLID,
        bAttr: TF_ATTR_INPUT,
        ..Default::default()
    }
}

#[implement(ITfDisplayAttributeInfo)]
struct Attribute {
    _module: ModuleRef,
    value: Cell<TF_DISPLAYATTRIBUTE>,
}

impl ITfDisplayAttributeInfo_Impl for Attribute_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(PREEDIT_ATTRIBUTE)
    }

    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("DIY IME preedit"))
    }

    fn GetAttributeInfo(&self, value: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if value.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { value.write(self.value.get()) };
        Ok(())
    }

    fn SetAttributeInfo(&self, value: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        if value.is_null() {
            return Err(E_POINTER.into());
        }
        self.value.set(unsafe { *value });
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        self.value.set(default_attribute());
        Ok(())
    }
}

pub(crate) fn attribute(guid: *const GUID) -> Result<ITfDisplayAttributeInfo> {
    if guid.is_null() {
        return Err(E_POINTER.into());
    }
    if unsafe { *guid } != PREEDIT_ATTRIBUTE {
        return Err(E_INVALIDARG.into());
    }
    Ok(Attribute {
        _module: ModuleRef::new(),
        value: Cell::new(default_attribute()),
    }
    .into())
}

#[implement(IEnumTfDisplayAttributeInfo)]
struct Attributes {
    _module: ModuleRef,
    exhausted: Cell<bool>,
}

pub(crate) fn attributes(exhausted: bool) -> IEnumTfDisplayAttributeInfo {
    Attributes {
        _module: ModuleRef::new(),
        exhausted: Cell::new(exhausted),
    }
    .into()
}

impl IEnumTfDisplayAttributeInfo_Impl for Attributes_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(attributes(self.exhausted.get()))
    }

    fn Next(
        &self,
        count: u32,
        info: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if (info.is_null() && count != 0) || (fetched.is_null() && count != 1) {
            return Err(E_POINTER.into());
        }
        let count_read = u32::from(count != 0 && !self.exhausted.replace(true));
        if count_read == 1 {
            unsafe { info.write(Some(attribute(&PREEDIT_ATTRIBUTE)?)) };
        }
        if !fetched.is_null() {
            unsafe { fetched.write(count_read) };
        }
        if count_read == count {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }

    fn Reset(&self) -> Result<()> {
        self.exhausted.set(false);
        Ok(())
    }

    fn Skip(&self, count: u32) -> Result<()> {
        if count == 0 || (!self.exhausted.replace(true) && count == 1) {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }
}
