//! The floating tool's model row. Choosing is explicit; merely drawing writes nothing.
use crate::sam3_route::SelectionModel;

use crate::sam3_route::model_store::{OptionEntry, Target};
use crate::sam3_selection::Runtime;

pub(super) fn show(ui: &mut egui::Ui, runtime: &mut Runtime) {
    let label = runtime.model_picker.snapshot.label.as_deref().unwrap_or(runtime.model_label()).to_owned();
    let options = if runtime.model_picker.snapshot.options.is_empty() {
        [SelectionModel::Sam3, SelectionModel::Sam21]
            .into_iter()
            .map(|model| OptionEntry {
                label: model.label().into(),
                target: Target::Local { workflow: if model == SelectionModel::Sam3 { "sam3-segment" } else { "sam21-segment" }.into() },
                reason: Some("The model list is loading".into()),
            })
            .collect()
    } else {
        runtime.model_picker.snapshot.options.clone()
    };
    let mut chosen = None;
    let response = egui::ComboBox::from_id_salt("object-selection-model").selected_text(&label).width(ui.available_width()).show_ui(ui, |ui| {
        for option in options {
            let text = if let Some(reason) = &option.reason { format!("{} ({reason})", option.label) } else { option.label.clone() };
            let selected = runtime.model_picker.snapshot.selected.as_ref() == Some(&option.target);
            if ui
                .add_enabled(option.reason.is_none() && !runtime.model_picker.busy && !runtime.model_picker.loading, egui::Button::selectable(selected, text))
                .clicked()
                && !selected
            {
                chosen = Some(option.target);
            }
        }
    });
    response.response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), &label));
    if response.response.clicked() {
        runtime.model_picker.refresh();
    }
    if let Some(target) = chosen {
        runtime.model_picker.request(target);
    }
}
