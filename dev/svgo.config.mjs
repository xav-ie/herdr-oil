// svgo for the VHS demos: everything in preset-default except removing hidden
// elements, since an animation's later frames start out hidden.
export default {
  plugins: [
    {
      name: "preset-default",
      params: {
        overrides: {
          removeHiddenElems: false,
        },
      },
    },
  ],
};
