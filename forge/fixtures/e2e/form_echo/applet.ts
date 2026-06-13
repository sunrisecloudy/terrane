type FormEchoInput = {
  name: string;
  email: string;
  message: string;
};

export async function main(ctx: any, input: FormEchoInput) {
  const tree = {
    type: "Stack",
    testId: "form-echo-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "form-title", text: "Form Echo", variant: "title" },
      { type: "TextField", testId: "field-name", label: "Name", value: input.name },
      { type: "TextField", testId: "field-email", label: "Email", value: input.email },
      { type: "Text", testId: "message", text: input.message }
    ]
  };

  ctx.ui.render(tree);
  return {
    ok: true,
    value: {
      name: input.name,
      email: input.email,
      message: input.message
    }
  };
}
