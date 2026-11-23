async function check(input, ctx) {
  const opened = await tools.Read({ file_path: input.workspace + "/demo/opened.txt" });
  return {
    pass: input.gate === "demo-check" && opened.content.trim() === input.run,
    detail: "checked " + input.gate + " at " + input.station + " via " + ctx.via,
  };
}

async function open(input, ctx) {
  await tools.Write({ file_path: input.workspace + "/demo/opened.txt", content: input.run + "\n" });
  return { change: "c-" + input.step };
}

async function defs(input, ctx) {
  return await codix.flow.defs();
}

async function start(input, ctx) {
  return await codix.flow.start({ flow: "demo-flow", subject: "from a gate" });
}
