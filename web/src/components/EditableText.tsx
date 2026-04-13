import { useState, useRef, useEffect } from "react";

interface Props {
  value: string;
  onSave: (value: string) => void;
  multiline?: boolean;
  className?: string;
  editClassName?: string;
  placeholder?: string;
}

export function EditableText({
  value,
  onSave,
  multiline = false,
  className = "",
  editClassName = "",
  placeholder = "Click to edit",
}: Props) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const ref = useRef<HTMLInputElement | HTMLTextAreaElement>(null);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  useEffect(() => {
    if (editing && ref.current) {
      ref.current.focus();
      ref.current.select();
    }
  }, [editing]);

  function save() {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed !== value) {
      onSave(trimmed);
    }
  }

  function cancel() {
    setDraft(value);
    setEditing(false);
  }

  if (!editing) {
    return (
      <span
        onClick={() => setEditing(true)}
        className={`cursor-pointer hover:bg-gray-800/50 rounded px-0.5 -mx-0.5 transition ${className}`}
        title="Click to edit"
      >
        {value || <span className="text-gray-600 italic">{placeholder}</span>}
      </span>
    );
  }

  const sharedProps = {
    value: draft,
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
      setDraft(e.target.value),
    onBlur: save,
    onKeyDown: (e: React.KeyboardEvent) => {
      if (e.key === "Escape") cancel();
      if (e.key === "Enter" && !multiline) save();
      if (e.key === "Enter" && multiline && e.metaKey) save();
    },
    className: `bg-gray-800 border border-indigo-600/50 rounded px-1.5 py-0.5 outline-none text-gray-100 w-full ${editClassName}`,
  };

  if (multiline) {
    return (
      <textarea
        ref={ref as React.RefObject<HTMLTextAreaElement>}
        rows={Math.max(3, draft.split("\n").length)}
        {...sharedProps}
      />
    );
  }

  return (
    <input
      ref={ref as React.RefObject<HTMLInputElement>}
      type="text"
      {...sharedProps}
    />
  );
}
