"""The trial's phrases.

EYE are the phrases iris.py asks of each eye crop, MOUTH the phrases
mouth.py asks of each mouth crop.

PRESETS are what the editor would offer from a menu; their vectors are
computed once (presets.py) and the trial runs them without the
language encoder. FREE are phrases a user might type into "Describe".
Neither list says what each frame holds; absences (teeth with closed
mouths, sky indoors, a dog nowhere) are read off the sheets per frame.
"""

PRESETS = [
    # the body's parts
    "face", "facial skin", "skin", "hair", "eyebrows", "eyes", "iris",
    "pupil", "eye whites", "eyelashes", "lips", "teeth", "hands", "beard",
    # clothing and accessories
    "sweater", "shirt", "jacket", "coat", "dress", "jeans", "trousers",
    "shoes", "boots", "hat", "sunglasses", "jewelry", "belt",
    # the scene
    "sky", "tree", "plants", "water", "wall", "floor",
]

FREE = [
    "leopard print coat", "blonde hair", "wine glass", "neon sign",
    "string lights", "the man's sunglasses", "the woman on the left",
    "wheat", "brick wall", "couch cushions",
    # absent from every frame
    "dog", "bicycle", "umbrella", "red car", "snow", "wedding dress",
]

EYE = ["iris", "pupil", "eye whites", "eyelashes", "eyes",
       "iris of the eye", "colored part of the eye", "eyeball", "blue iris", "sclera"]

MOUTH = ["teeth", "lips", "upper lip", "lower lip", "mouth", "gums",
         "tongue", "smile", "white teeth"]
