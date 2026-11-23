// Übernimmt die Befehle der Desktop-App (src-tauri/src/lib.rs) für den NAS-Server:
// - entfernt Fenster/Infobereich/Tabs (gibt es im Browser nicht)
// - erzeugt den Verteiler, der Aufrufe aus dem Browser (POST /api/invoke/<befehl>) an dieselben Funktionen gibt.
// So bleibt die NAS-Version automatisch auf dem Stand der Desktop-App.
use quote::{ToTokens, quote};
use std::fmt::Write;

/// Gibt es im Browser nicht – werden in server/src/stubs.rs ersetzt
const DROP: &[&str] = &[
    "run", "setup_tray", "tray_menu", "set_tray_title", "quit_app", "show_main", "tab_window", "tab_show", "show_tab",
    "tab_create", "tab_rename", "tab_close", "tabs_list", "allow_device_names", "set_lang", "quit", "handle_deep_link",
];

fn is_command(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::") == "tauri::command")
}

fn camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn main() {
    let src = "../src-tauri/src/lib.rs";
    println!("cargo:rerun-if-changed={src}");
    let text = std::fs::read_to_string(src).expect("lib.rs");
    let mut file: syn::File = syn::parse_file(&text).expect("lib.rs parsen");
    let mut dispatch = String::new();
    let mut names = Vec::new();

    file.items.retain_mut(|item| match item {
        syn::Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            if DROP.contains(&name.as_str()) {
                return false;
            }
            // Attribute wie #[cfg_attr(mobile, …)] / #[cfg(windows)] entfernen bzw. Fenster-Code weglassen
            if f.attrs.iter().any(|a| a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("windows")) {
                return false;
            }
            if is_command(&f.attrs) {
                f.attrs.retain(|a| !a.path().segments.iter().any(|s| s.ident == "tauri"));
                f.attrs.push(syn::parse_quote!(#[allow(dead_code)]));
                // Verteiler-Zweig
                let mut args = Vec::new();
                let mut lets = String::new();
                for (i, inp) in f.sig.inputs.iter().enumerate() {
                    let syn::FnArg::Typed(pt) = inp else { continue };
                    let pname = match &*pt.pat {
                        syn::Pat::Ident(p) => p.ident.to_string(),
                        _ => format!("a{i}"),
                    };
                    let ty = pt.ty.to_token_stream().to_string();
                    let tyc = ty.replace(' ', "");
                    let var = format!("a{i}");
                    let expr = if tyc == "AppHandle" || tyc == "tauri::AppHandle" {
                        "app.clone()".to_string()
                    } else if tyc == "Tab" {
                        "Tab(tab_state(app, \"main\").map_err(err)?)".to_string()
                    } else if tyc.contains("WebviewWindow") || tyc.contains("Window") {
                        "tauri::WebviewWindow::main(app)".to_string()
                    } else {
                        format!("crate::web::arg::<{ty}>(args, \"{}\")?", camel(&pname))
                    };
                    writeln!(lets, "            let {var} = {expr};").unwrap();
                    args.push(var);
                }
                let call = format!("{name}({}){}", args.join(", "), if f.sig.asyncness.is_some() { ".await" } else { "" });
                let ret = match &f.sig.output {
                    syn::ReturnType::Default => "unit",
                    syn::ReturnType::Type(_, t) => {
                        let s = t.to_token_stream().to_string().replace(' ', "");
                        if s.starts_with("R<") || s.starts_with("Result<") { "result" } else { "value" }
                    }
                };
                let body = match ret {
                    "unit" => format!("{{ {call}; Ok(serde_json::Value::Null) }}"),
                    "result" => format!("{{ match {call} {{ Ok(v) => Ok(serde_json::to_value(v).map_err(err)?), Err(e) => Err(e.to_string()) }} }}"),
                    _ => format!("{{ Ok(serde_json::to_value({call}).map_err(err)?) }}"),
                };
                writeln!(dispatch, "        \"{name}\" => {{\n{lets}            {body}\n        }}").unwrap();
                names.push(name);
            }
            true
        }
        // Befehlsargument „Tab“ kommt im Server vom Verteiler
        syn::Item::Impl(im) => !im.to_token_stream().to_string().contains("CommandArg"),
        _ => true,
    });
    // #[cfg_attr(mobile, tauri::mobile_entry_point)] o. ä. an verbliebenen Elementen entfernen
    let mut out = file.to_token_stream().to_string();
    out = out.replace("# [cfg_attr (mobile , tauri :: mobile_entry_point)]", "");

    let generated = format!(
        "// Erzeugt von server/build.rs aus src-tauri/src/lib.rs – nicht von Hand bearbeiten\n{out}\n\n\
         /// Alle Befehle der Oberfläche\n\
         pub(crate) async fn dispatch(app: &AppHandle, name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {{\n\
         #[allow(unused_variables)]\n    match name {{\n{dispatch}        other => crate::stubs::call(app, other, args).await,\n    }}\n}}\n\
         pub(crate) const COMMANDS: &[&str] = &[{}];\n",
        names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
    );
    let _ = quote!(); // quote wird für ToTokens gebraucht
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("app.rs");
    std::fs::write(dest, generated).unwrap();
}
