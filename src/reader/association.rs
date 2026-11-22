use super::{HWND, Reader, RefCell, SW_SHOWNORMAL};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use windows::Win32::Storage::Packaging::Appx::{
    GetCurrentApplicationUserModelId, GetCurrentPackageFamilyName,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_NONE, REG_SZ, RegSetKeyValueW};
use windows::Win32::UI::Shell::{
    ASSOCF_NONE, ASSOCSTR, ASSOCSTR_APPID, ASSOCSTR_EXECUTABLE, ASSOCSTR_PROGID, AssocQueryStringW,
    SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify, ShellExecuteW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};

const PROG_ID: &str = "ScoreDraw.PDF";
const CAPABILITIES: &str = r"Software\ScoreDraw\Capabilities";
const SETTINGS: &str = "ms-settings:defaultapps";

pub(super) fn prompt(hwnd: HWND, state: &RefCell<Reader>, manual: bool) {
    if !manual && state.borrow().settings.pdf_default_prompt_seen {
        return;
    }
    let app_id = package_app_id();
    if is_default(app_id.as_deref()) {
        remember_prompt(hwnd, state);
        if manual {
            state.borrow_mut().notify(
                hwnd,
                tr!(
                    "ScoreDraw è già l’app predefinita per i PDF.",
                    "ScoreDraw is already your default PDF app."
                )
                .into(),
            );
        }
        return;
    }
    match crate::dialog::default_pdf(hwnd) {
        Ok(open_settings) => {
            remember_prompt(hwnd, state);
            if open_settings && let Err(error) = open_default_settings(hwnd, app_id.as_deref()) {
                state.borrow().show_error(
                    hwnd,
                    &trf!(
                        "Impossibile aprire la scelta dell’app predefinita. {error}",
                        "Couldn’t open the default app settings. {error}"
                    ),
                );
            }
        }
        Err(error) => state.borrow().show_error(hwnd, &error.to_string()),
    }
}

fn remember_prompt(hwnd: HWND, state: &RefCell<Reader>) {
    let mut app = state.borrow_mut();
    if !app.settings.pdf_default_prompt_seen {
        let settings = crate::data::Settings {
            pdf_default_prompt_seen: true,
            ..app.settings
        };
        app.commit_settings(hwnd, settings);
    }
}

fn package_app_id() -> Option<String> {
    let mut value = [0; 256];
    let mut len = value.len() as u32;
    if unsafe { GetCurrentApplicationUserModelId(&mut len, Some(PWSTR(value.as_mut_ptr()))) }
        .is_ok()
    {
        return Some(String::from_utf16_lossy(
            &value[..len.saturating_sub(1) as usize],
        ));
    }
    len = value.len() as u32;
    unsafe { GetCurrentPackageFamilyName(&mut len, Some(PWSTR(value.as_mut_ptr()))) }
        .ok()
        .ok()?;
    let family = String::from_utf16_lossy(&value[..len.saturating_sub(1) as usize]);
    Some(format!("{family}!ScoreDraw"))
}

fn pdf_association(kind: ASSOCSTR) -> Option<String> {
    let mut value = [0; 1024];
    let mut len = value.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            kind,
            w!(".pdf"),
            PCWSTR::null(),
            Some(PWSTR(value.as_mut_ptr())),
            &mut len,
        )
    }
    .ok()
    .ok()?;
    let content = value.split(|unit| *unit == 0).next()?;
    (!content.is_empty()).then(|| String::from_utf16_lossy(content))
}

fn is_default(app_id: Option<&str>) -> bool {
    if let Some(app_id) = app_id {
        return pdf_association(ASSOCSTR_APPID)
            .is_some_and(|current| current.eq_ignore_ascii_case(app_id));
    }
    if !pdf_association(ASSOCSTR_PROGID)
        .is_some_and(|current| current.eq_ignore_ascii_case(PROG_ID))
    {
        return false;
    }
    let Some(current) = pdf_association(ASSOCSTR_EXECUTABLE) else {
        return false;
    };
    let associated = Path::new(&current).canonicalize();
    let executable = std::env::current_exe().and_then(|path| path.canonicalize());
    matches!((associated, executable), (Ok(associated), Ok(executable)) if associated == executable)
}

fn open_default_settings(hwnd: HWND, app_id: Option<&str>) -> Result<()> {
    let uri = if let Some(id) = app_id {
        format!("{SETTINGS}?registeredAUMID={}", id.replace('!', "%21"))
    } else {
        register_portable()?;
        format!("{SETTINGS}?registeredAppUser=ScoreDraw")
    };
    if launch(hwnd, &uri) || launch(hwnd, SETTINGS) {
        Ok(())
    } else {
        Err(io::Error::other(tr!(
            "Le Impostazioni di Windows non si sono aperte.",
            "Windows Settings didn’t open."
        ))
        .into())
    }
}

fn launch(hwnd: HWND, uri: &str) -> bool {
    let result = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            &HSTRING::from(uri),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as isize) > 32
}

fn register_portable() -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut quoted = OsString::from("\"");
    quoted.push(executable);
    quoted.push("\"");
    let with_suffix = |suffix: &str| {
        let mut value = quoted.clone();
        value.push(suffix);
        value
    };
    let description = OsStr::new(tr!(
        "Il lettore PDF pensato per i musicisti",
        "The PDF reader designed for musicians"
    ));
    let prog_id = OsStr::new(PROG_ID);
    let class = format!(r"Software\Classes\{PROG_ID}");

    set_value(
        &class,
        "",
        Some(OsStr::new(tr!(
            "Documento PDF · ScoreDraw",
            "PDF document · ScoreDraw"
        ))),
    )?;
    set_value(
        &format!(r"{class}\DefaultIcon"),
        "",
        Some(&with_suffix(",-102")),
    )?;
    set_value(
        &format!(r"{class}\shell\open\command"),
        "",
        Some(&with_suffix(" \"%1\"")),
    )?;
    set_value(
        CAPABILITIES,
        "ApplicationName",
        Some(OsStr::new("ScoreDraw")),
    )?;
    set_value(CAPABILITIES, "ApplicationDescription", Some(description))?;
    set_value(CAPABILITIES, "ApplicationIcon", Some(&with_suffix(",-101")))?;
    set_value(
        &format!(r"{CAPABILITIES}\FileAssociations"),
        ".pdf",
        Some(prog_id),
    )?;
    set_value(r"Software\Classes\.pdf\OpenWithProgids", PROG_ID, None)?;
    set_value(
        r"Software\RegisteredApplications",
        "ScoreDraw",
        Some(OsStr::new(CAPABILITIES)),
    )?;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

fn set_value(key: &str, name: &str, value: Option<&OsStr>) -> Result<()> {
    let value = value.map(HSTRING::from);
    let (kind, pointer, size) = value.as_ref().map_or((REG_NONE, None, 0), |value| {
        (
            REG_SZ,
            Some(value.as_ptr().cast()),
            ((value.len() + 1) * 2) as u32,
        )
    });
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            kind.0,
            pointer,
            size,
        )
    }
    .ok()
}
