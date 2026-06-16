use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(|| {
        let id = "01JZ8K3Q9FXM2H7B4N0PXR5T6A".parse::<paavo_proto::JobId>();
        let text = match id {
            Ok(j) => j.to_string(),
            Err(_) => "parse-failed".into(),
        };
        view! { <p>"hello paavo: " {text}</p> }
    });
}
