use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use windows::Win32::{
    Foundation::{BOOL, E_INVALIDARG, E_NOINTERFACE, E_POINTER, E_UNEXPECTED, POINT, RECT},
    System::Variant::{VT_EMPTY, VT_I4},
    UI::{
        TextServices::{
            ITfCompartment, ITfCompartmentEventSink, ITfCompartmentEventSink_Impl, ITfLangBarItem,
            ITfLangBarItem_Impl, ITfLangBarItemButton, ITfLangBarItemButton_Impl,
            ITfLangBarItemMgr, ITfLangBarItemSink, ITfMenu, ITfSource, ITfSource_Impl,
            ITfThreadMgr, TF_LANGBARITEMINFO, TF_LBI_CLK_LEFT, TF_LBI_STATUS, TF_LBI_STATUS_HIDDEN,
            TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY, TF_LBI_TEXT, TF_LBI_TOOLTIP,
            TfLBIClick,
        },
        WindowsAndMessaging::{CopyIcon, HICON, IDI_INFORMATION, LoadIconW},
    },
};
use windows_core::{BSTR, GUID, IUnknown, Interface, Result, VARIANT, implement};

use crate::{ModuleRef, TEST_SERVICE_CLSID, service::Activation};

pub const PROTECTION_ITEM: GUID = GUID::from_u128(0x771456f7_48f9_4d38_92d8_2b968d86224d);
const PROTECTION_STATE: GUID = GUID::from_u128(0xcf33b3ad_4ec8_483e_a2b5_0b77e37cd744);

pub(crate) struct ManualProtection {
    control: Rc<Control>,
    subscription: (ITfSource, u32),
    button: Option<(ITfLangBarItemMgr, ITfLangBarItem)>,
}

impl ManualProtection {
    pub(crate) fn open(manager: &ITfThreadMgr, owner: &Rc<Activation>) -> Result<Self> {
        let compartment = unsafe {
            manager
                .GetGlobalCompartment()?
                .GetCompartment(&PROTECTION_STATE)?
        };
        let control = Rc::new(Control {
            compartment,
            visible: Cell::new(true),
            sink: RefCell::new(None),
        });
        let source: ITfSource = control.compartment.cast()?;
        let observer: ITfCompartmentEventSink = Observer {
            _module: ModuleRef::new(),
            owner: Rc::downgrade(owner),
            control: control.clone(),
        }
        .into();
        let identity: IUnknown = observer.cast()?;
        let cookie = unsafe { source.AdviseSink(&ITfCompartmentEventSink::IID, &identity)? };
        let mut registration = Self {
            control,
            subscription: (source, cookie),
            button: None,
        };
        if let Ok(items) = manager.cast::<ITfLangBarItemMgr>() {
            let button: ITfLangBarItemButton = ProtectionButton {
                _module: ModuleRef::new(),
                owner: Rc::downgrade(owner),
                control: registration.control.clone(),
            }
            .into();
            let item: ITfLangBarItem = button.cast()?;
            if unsafe { items.AddItem(&item).is_ok() } {
                registration.button = Some((items, item));
            }
        }
        Ok(registration)
    }

    pub(crate) fn blocked(&self) -> bool {
        self.control.read().unwrap_or(true)
    }
}

impl Drop for ManualProtection {
    fn drop(&mut self) {
        let (source, cookie) = &self.subscription;
        let _ = unsafe { source.UnadviseSink(*cookie) };
        if let Some((manager, item)) = self.button.take() {
            let _ = unsafe { manager.RemoveItem(&item) };
        }
    }
}

struct Control {
    compartment: ITfCompartment,
    visible: Cell<bool>,
    sink: RefCell<Option<ITfLangBarItemSink>>,
}

impl Control {
    fn read(&self) -> Option<bool> {
        let value = unsafe { self.compartment.GetValue().ok()? };
        let fields = unsafe { value.as_raw().Anonymous.Anonymous };
        match fields.vt {
            tag if tag == VT_EMPTY.0 => Some(false),
            tag if tag == VT_I4.0 => match unsafe { fields.Anonymous.lVal } {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            _ => None,
        }
    }

    fn notify(&self) {
        let sink = self.sink.borrow().clone();
        if let Some(sink) = sink {
            let _ = unsafe { sink.OnUpdate(TF_LBI_TEXT | TF_LBI_TOOLTIP | TF_LBI_STATUS) };
        }
    }
}

#[implement(ITfCompartmentEventSink)]
struct Observer {
    _module: ModuleRef,
    owner: Weak<Activation>,
    control: Rc<Control>,
}

impl ITfCompartmentEventSink_Impl for Observer_Impl {
    fn OnChange(&self, guid: *const GUID) -> Result<()> {
        if guid.is_null() || unsafe { *guid } != PROTECTION_STATE {
            return Ok(());
        }
        if self.control.read().unwrap_or(true)
            && let Some(owner) = self.owner.upgrade()
        {
            owner.protect();
        }
        self.control.notify();
        Ok(())
    }
}

#[implement(ITfLangBarItemButton, ITfSource)]
struct ProtectionButton {
    _module: ModuleRef,
    owner: Weak<Activation>,
    control: Rc<Control>,
}

impl ITfLangBarItem_Impl for ProtectionButton_Impl {
    fn GetInfo(&self, info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if info.is_null() {
            return Err(E_POINTER.into());
        }
        let mut description = [0; 32];
        for (position, character) in "DIY IME 保護入力".encode_utf16().enumerate() {
            description[position] = character;
        }
        unsafe {
            info.write(TF_LANGBARITEMINFO {
                clsidService: TEST_SERVICE_CLSID,
                guidItem: PROTECTION_ITEM,
                dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
                ulSort: 0,
                szDescription: description,
            });
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(if self.control.visible.get() {
            0
        } else {
            TF_LBI_STATUS_HIDDEN
        })
    }

    fn Show(&self, show: BOOL) -> Result<()> {
        self.control.visible.set(show.as_bool());
        self.control.notify();
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<BSTR> {
        self.GetText()
    }
}

impl ITfLangBarItemButton_Impl for ProtectionButton_Impl {
    fn OnClick(&self, click: TfLBIClick, _: &POINT, _: *const RECT) -> Result<()> {
        if click != TF_LBI_CLK_LEFT {
            return Ok(());
        }
        let owner = self
            .owner
            .upgrade()
            .filter(|owner| owner.active())
            .ok_or(E_UNEXPECTED)?;
        let enabled = !self.control.read().unwrap_or(true);
        if enabled {
            owner.protect();
        }
        unsafe {
            self.control
                .compartment
                .SetValue(owner.client_id, &VARIANT::from(i32::from(enabled)))?
        };
        self.control.notify();
        Ok(())
    }

    fn InitMenu(&self, _: Option<&ITfMenu>) -> Result<()> {
        Ok(())
    }

    fn OnMenuSelect(&self, _: u32) -> Result<()> {
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        unsafe { CopyIcon(LoadIconW(None, IDI_INFORMATION)?) }
    }

    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::from(if self.control.read().unwrap_or(true) {
            "保護入力 オン"
        } else {
            "保護入力 オフ"
        }))
    }
}

impl ITfSource_Impl for ProtectionButton_Impl {
    fn AdviseSink(&self, interface: *const GUID, sink: Option<&IUnknown>) -> Result<u32> {
        if interface.is_null() || unsafe { *interface } != ITfLangBarItemSink::IID {
            return Err(E_NOINTERFACE.into());
        }
        let sink: ITfLangBarItemSink = sink.ok_or(E_POINTER)?.cast()?;
        let mut slot = self
            .control
            .sink
            .try_borrow_mut()
            .map_err(|_| E_UNEXPECTED)?;
        if slot.is_some() {
            return Err(E_UNEXPECTED.into());
        }
        *slot = Some(sink);
        Ok(1)
    }

    fn UnadviseSink(&self, cookie: u32) -> Result<()> {
        if cookie != 1 || self.control.sink.take().is_none() {
            return Err(E_INVALIDARG.into());
        }
        Ok(())
    }
}
