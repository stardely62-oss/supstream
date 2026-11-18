// Панель (ui/dist) вшивается через rust-embed. Если UI ещё не собран,
// кладём заглушку, чтобы ядро компилировалось и без Node.js.
fn main() {
    let dist = std::path::Path::new("../../ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(dist).expect("не удалось создать ui/dist");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><meta charset=utf-8><title>Supstream</title>\
             <p style=\"font-family:sans-serif\">Панель не собрана: выполните <code>pnpm --dir ui build</code>.</p>",
        )
        .expect("не удалось записать заглушку панели");
    }
    println!("cargo:rerun-if-changed=../../ui/dist");
}
