// A still today, a clip the moment one exists.
//
// Every feature figure on these pages is a real screenshot of the window,
// marked with the clip that belongs there. This looks for that clip and, where
// it finds one, swaps the picture for a video with the picture as its poster.
//
// Written this way round on purpose. A page full of <video> tags pointing at
// files nobody has recorded yet is a page full of broken players, and the
// alternative — editing the HTML when each capture lands — is a step somebody
// has to remember. Dropping an .mp4 into site/media/ is the whole job.
//
// With scripting off, the stills stay, which is a page that still says what the
// program does.

(() => {
  const figures = document.querySelectorAll("figure[data-clip]");
  if (!figures.length || !window.fetch) return;

  for (const figure of figures) {
    const clip = figure.getAttribute("data-clip");
    const still = figure.querySelector("img");
    if (!clip || !still) continue;

    // HEAD rather than letting the element ask for it: a <video> whose source
    // 404s reports nothing useful and leaves the player showing its own idea
    // of an error.
    fetch(clip, { method: "HEAD" })
      .then((answer) => {
        if (!answer.ok) return;

        const video = document.createElement("video");
        video.src = clip;
        video.poster = still.getAttribute("src");
        video.controls = true;
        video.playsInline = true;
        video.preload = "none";
        // Everything shown here is a program being used, so there is nothing
        // to hear and no reason to ask before loading megabytes of it.
        video.muted = true;
        video.setAttribute("aria-label", still.getAttribute("alt") || "");

        still.replaceWith(video);
        // A crop frames part of a still; a clip is shown whole.
        figure.querySelector(".shot")?.classList.remove("crop");

        // The caption says a clip is still to be recorded. It is not.
        figure.querySelector("figcaption .await")?.remove();
      })
      .catch(() => {
        /* No clip, or nothing answering. The still is already right. */
      });
  }
})();
